//! The tools on Tjommie's MCP server (`Team.tools()` in `team.ts`): `request_login`, `list_bots`, `message_bot`,
//! `share_file` and `add_routine`. Each tool's arguments are checked first, with the text zod's `prettifyError` would
//! give, and then run as the calling bot on the runner's host.
//!
//! A tool body returns the text the bot reads, or an error whose message the bot reads. The bodies are public methods of
//! [`Team`], so tests can call them on a fake host without going through the MCP endpoint.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use protocol::attachments::{
    AttachmentCompanion, AttachmentCompanions, CompanionName, MAX_ATTACHMENT_COMPANIONS,
};
use protocol::model::{
    ApprovalKind, ApprovalStatus, ExchangeHandoffData, ExchangePostData, ExchangeStatus,
    RoutineApprovalData, RoutineInput, RoutineKind, TimeZone,
};
use serde_json::{Map, Value, json};

use super::files::{snapshot_file, within};
use super::*;
use crate::approvals::{NewApproval, create_approval};
use crate::mcp::{ArgIssue, McpTool, ToolFuture, prettify};
use crate::queue::{get_work_item, use_chain_request};
use crate::routines::host_time_zone;
use crate::scheduler::duration;
use crate::settings::get_settings;

const MAX_PENDING_ROUTINE_PROPOSALS: i64 = 5;
const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// One companion image a `share_file` call names: its name in the Markdown and where its source is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionArg {
    pub name: CompanionName,
    pub path: String,
}

/// `add_routine`'s arguments after checking: the person's schedule, with days as numbers (0 is Sunday).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineProposal {
    pub name: String,
    pub days: Vec<u8>,
    pub times: Vec<String>,
    pub prompt: String,
    pub time_limit_minutes: Option<u32>,
}

// ---- checking arguments, the way zod reports them ----

/// zod's name for the kind of value it received.
fn received(value: Option<&Value>) -> &'static str {
    match value {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

fn issue(path: &[&str], message: impl Into<String>) -> ArgIssue {
    ArgIssue {
        path: path.iter().map(|segment| (*segment).to_owned()).collect(),
        message: message.into(),
    }
}

/// `z.object` (`strict` is `z.strictObject`): the arguments must be an object, and a strict one has no other keys.
fn object_of<'a>(
    arguments: &'a Value,
    strict: Option<&[&str]>,
    issues: &mut Vec<ArgIssue>,
) -> Option<&'a Map<String, Value>> {
    let Some(object) = arguments.as_object() else {
        issues.push(issue(
            &[],
            format!(
                "Invalid input: expected object, received {}",
                received(Some(arguments))
            ),
        ));
        return None;
    };
    if let Some(allowed) = strict {
        let unknown: Vec<String> = object
            .keys()
            .filter(|key| !allowed.contains(&key.as_str()))
            .map(|key| format!("\"{key}\""))
            .collect();
        if !unknown.is_empty() {
            let label = if unknown.len() == 1 { "key" } else { "keys" };
            issues.push(issue(
                &[],
                format!("Unrecognized {label}: {}", unknown.join(", ")),
            ));
        }
    }
    Some(object)
}

/// `z.string()`, with `.trim()` when asked, and its length bounds in UTF-16 units. Reports every bound it breaks.
fn string_at(
    value: Option<&Value>,
    path: &[&str],
    trim: bool,
    min: usize,
    max: Option<usize>,
    issues: &mut Vec<ArgIssue>,
) -> Option<String> {
    let Some(Value::String(raw)) = value else {
        issues.push(issue(
            path,
            format!(
                "Invalid input: expected string, received {}",
                received(value)
            ),
        ));
        return None;
    };
    let text = if trim { js::trim(raw) } else { raw.as_str() };
    let length = js::utf16_len(text);
    let mut valid = true;
    if length < min {
        issues.push(issue(
            path,
            format!("Too small: expected string to have >={min} characters"),
        ));
        valid = false;
    }
    if let Some(max) = max
        && length > max
    {
        issues.push(issue(
            path,
            format!("Too big: expected string to have <={max} characters"),
        ));
        valid = false;
    }
    valid.then(|| text.to_owned())
}

/// `z.array(..)` with its item bounds. `None` after reporting when the value is not an array.
fn array_at<'a>(
    value: Option<&'a Value>,
    path: &[&str],
    min: usize,
    max: usize,
    issues: &mut Vec<ArgIssue>,
) -> Option<&'a Vec<Value>> {
    let Some(Value::Array(items)) = value else {
        issues.push(issue(
            path,
            format!(
                "Invalid input: expected array, received {}",
                received(value)
            ),
        ));
        return None;
    };
    if items.len() < min {
        issues.push(issue(
            path,
            format!("Too small: expected array to have >={min} items"),
        ));
    }
    if items.len() > max {
        issues.push(issue(
            path,
            format!("Too big: expected array to have <={max} items"),
        ));
    }
    Some(items)
}

/// `^([01]\d|2[0-3]):[0-5]\d$`.
fn is_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 5
        && bytes[2] == b':'
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && (text[..2].parse::<u8>().is_ok_and(|hour| hour <= 23))
        && bytes[3] <= b'5'
}

fn finished<T>(issues: Vec<ArgIssue>, value: Option<T>) -> Result<T, String> {
    match (issues.is_empty(), value) {
        (true, Some(value)) => Ok(value),
        _ => Err(prettify(&issues)),
    }
}

fn parse_request_login(arguments: &Value) -> Result<String, String> {
    let mut issues = Vec::new();
    let object = object_of(arguments, Some(&["name"]), &mut issues);
    let name = object.and_then(|object| {
        let name = string_at(
            object.get("name"),
            &["name"],
            true,
            1,
            Some(100),
            &mut issues,
        )?;
        if name.chars().any(|c| c.is_ascii_control()) {
            issues.push(issue(
                &["name"],
                "Invalid string: must match pattern /^[^\\x00-\\x1f\\x7f]+$/",
            ));
            return None;
        }
        Some(name)
    });
    finished(issues, name)
}

fn parse_no_arguments(arguments: &Value) -> Result<(), String> {
    let mut issues = Vec::new();
    object_of(arguments, None, &mut issues);
    finished(issues, Some(()))
}

fn parse_message_bot(arguments: &Value) -> Result<(String, String), String> {
    let mut issues = Vec::new();
    let object = object_of(arguments, None, &mut issues);
    let parsed = object.and_then(|object| {
        let bot = string_at(object.get("bot"), &["bot"], true, 1, None, &mut issues);
        let message = string_at(
            object.get("message"),
            &["message"],
            true,
            1,
            Some(20_000),
            &mut issues,
        );
        Some((bot?, message?))
    });
    finished(issues, parsed)
}

type ShareArgs = (String, Option<String>, Vec<CompanionArg>);

fn parse_share_file(arguments: &Value) -> Result<ShareArgs, String> {
    let mut issues = Vec::new();
    let object = object_of(arguments, None, &mut issues);
    let parsed = object.and_then(|object| {
        let path = string_at(object.get("path"), &["path"], false, 1, None, &mut issues);
        let caption = match object.get("caption") {
            None => Some(None),
            value => string_at(value, &["caption"], false, 0, Some(500), &mut issues).map(Some),
        };
        let companions = match object.get("companions") {
            None => Some(Vec::new()),
            value => {
                let items = array_at(
                    value,
                    &["companions"],
                    0,
                    MAX_ATTACHMENT_COMPANIONS,
                    &mut issues,
                );
                items.and_then(|items| {
                    let mut parsed = Vec::new();
                    let before = issues.len();
                    for (index, item) in items.iter().enumerate() {
                        let at = format!("companions[{index}]");
                        let entry = object_of_item(item, &at, &mut issues);
                        if let Some(entry) = entry {
                            let name = match entry.get("name") {
                                Some(Value::String(name)) => match CompanionName::new(name) {
                                    Ok(name) => Some(name),
                                    Err(error) => {
                                        issues.push(issue(&[&at, "name"], error.to_string()));
                                        None
                                    }
                                },
                                other => {
                                    issues.push(issue(
                                        &[&at, "name"],
                                        format!(
                                            "Invalid input: expected string, received {}",
                                            received(other)
                                        ),
                                    ));
                                    None
                                }
                            };
                            let path = string_at(
                                entry.get("path"),
                                &[&at, "path"],
                                false,
                                1,
                                Some(32_767),
                                &mut issues,
                            );
                            if let (Some(name), Some(path)) = (name, path) {
                                parsed.push(CompanionArg { name, path });
                            }
                        }
                    }
                    (issues.len() == before).then_some(parsed)
                })
            }
        };
        Some((path?, caption?, companions?))
    });
    finished(issues, parsed)
}

/// A companion entry: a strict object with `name` and `path`.
fn object_of_item<'a>(
    item: &'a Value,
    at: &str,
    issues: &mut Vec<ArgIssue>,
) -> Option<&'a Map<String, Value>> {
    let Some(object) = item.as_object() else {
        issues.push(issue(
            &[at],
            format!(
                "Invalid input: expected object, received {}",
                received(Some(item))
            ),
        ));
        return None;
    };
    let unknown: Vec<String> = object
        .keys()
        .filter(|key| !matches!(key.as_str(), "name" | "path"))
        .map(|key| format!("\"{key}\""))
        .collect();
    if !unknown.is_empty() {
        let label = if unknown.len() == 1 { "key" } else { "keys" };
        issues.push(issue(
            &[at],
            format!("Unrecognized {label}: {}", unknown.join(", ")),
        ));
    }
    Some(object)
}

fn parse_add_routine(arguments: &Value) -> Result<RoutineProposal, String> {
    let mut issues = Vec::new();
    let object = object_of(arguments, None, &mut issues);
    let parsed = object.and_then(|object| {
        let name = string_at(object.get("name"), &["name"], true, 1, Some(60), &mut issues);
        let days = array_at(object.get("days"), &["days"], 1, 7, &mut issues).and_then(|items| {
            let mut days = Vec::new();
            let mut valid = true;
            for (index, item) in items.iter().enumerate() {
                let day = item.as_str().and_then(|day| DAYS.iter().position(|known| *known == day));
                match day {
                    Some(day) => days.push(day as u8),
                    None => {
                        issues.push(issue(
                            &[&format!("days[{index}]")],
                            format!(
                                "Invalid option: expected one of {}",
                                DAYS.iter().map(|day| format!("\"{day}\"")).collect::<Vec<_>>().join("|")
                            ),
                        ));
                        valid = false;
                    }
                }
            }
            (valid && !items.is_empty() && items.len() <= 7).then_some(days)
        });
        let times = array_at(object.get("times"), &["times"], 1, 12, &mut issues).and_then(|items| {
            let mut times = Vec::new();
            let mut valid = true;
            for (index, item) in items.iter().enumerate() {
                match item.as_str().filter(|time| is_time(time)) {
                    Some(time) => times.push(time.to_owned()),
                    None => {
                        let message = match item {
                            Value::String(_) => "Invalid string: must match pattern /^([01]\\d|2[0-3]):[0-5]\\d$/".to_owned(),
                            other => format!("Invalid input: expected string, received {}", received(Some(other))),
                        };
                        issues.push(issue(&[&format!("times[{index}]")], message));
                        valid = false;
                    }
                }
            }
            (valid && !items.is_empty() && items.len() <= 12).then_some(times)
        });
        let prompt = string_at(object.get("prompt"), &["prompt"], true, 1, Some(10_000), &mut issues);
        let limit = match object.get("time_limit_minutes") {
            None => Some(None),
            Some(Value::Number(number)) => {
                let value = number.as_f64().unwrap_or(f64::NAN);
                if value.fract() != 0.0 {
                    issues.push(issue(&["time_limit_minutes"], "Invalid input: expected int, received number"));
                    None
                } else if value < 1.0 {
                    issues.push(issue(&["time_limit_minutes"], "Too small: expected number to be >=1"));
                    None
                } else if value > 1440.0 {
                    issues.push(issue(&["time_limit_minutes"], "Too big: expected number to be <=1440"));
                    None
                } else {
                    Some(Some(value as u32))
                }
            }
            other => {
                issues.push(issue(
                    &["time_limit_minutes"],
                    format!("Invalid input: expected number, received {}", received(other)),
                ));
                None
            }
        };
        Some(RoutineProposal {
            name: name?,
            days: days?,
            times: times?,
            prompt: prompt?,
            time_limit_minutes: limit?,
        })
    });
    finished(issues, parsed)
}

// ---- the tool list ----

/// A tool that runs on the host: on the blocking pool, since it takes the runner's locks.
fn host_tool<A: Send + 'static>(
    team: &Arc<Team>,
    access: &Arc<dyn HostAccess>,
    name: &str,
    description: &str,
    schema: Value,
    parse: impl Fn(&Value) -> Result<A, String> + Send + Sync + 'static,
    run: fn(&Team, &mut dyn TeamHost, &str, A) -> Result<String, RpcError>,
) -> McpTool {
    let (team, access) = (team.clone(), access.clone());
    McpTool::define(
        name,
        description,
        schema,
        parse,
        move |bot_id: String, args: A| -> ToolFuture {
            let (team, access) = (team.clone(), access.clone());
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    with_host(&*access, |host| run(&team, host, &bot_id, args))
                })
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.message)
            })
        },
    )
}

impl Team {
    /// The tools on Tjommie's MCP server. `bot_id` is the calling bot, known from its session's token.
    pub fn tools(self: &Arc<Self>, access: Arc<dyn HostAccess>) -> Vec<McpTool> {
        vec![
            host_tool(
                self,
                &access,
                "request_login",
                "Asks the person to select and confirm a named saved login in Logins. This never reads credentials or fills a website; browser automation stays under the person's control.",
                json!({
                    "type": "object",
                    "properties": { "name": { "type": "string", "minLength": 1, "maxLength": 100, "pattern": "^[^\\x00-\\x1f\\x7f]+$", "description": "The website login name to ask the person for; never a password" } },
                    "required": ["name"],
                    "additionalProperties": false,
                }),
                parse_request_login,
                |team, host, bot_id, name| team.request_login(host, bot_id, &name),
            ),
            host_tool(
                self,
                &access,
                "list_bots",
                "Lists the bots you may ask for help with message_bot: their names, what they do, and whether they're busy.",
                json!({ "type": "object", "properties": {} }),
                parse_no_arguments,
                |team, host, bot_id, ()| team.list_bots(host, bot_id),
            ),
            host_tool(
                self,
                &access,
                "message_bot",
                "Sends a request to another bot and returns at once. The bot works on it when it's free, and its answer arrives later as a new message to you. Don't wait or check for it.",
                json!({
                    "type": "object",
                    "properties": {
                        "bot": { "type": "string", "minLength": 1, "description": "The bot's name, as list_bots shows it" },
                        "message": { "type": "string", "minLength": 1, "maxLength": 20000, "description": "What you want, with everything the bot needs to know" },
                    },
                    "required": ["bot", "message"],
                }),
                parse_message_bot,
                |team, host, bot_id, (bot, message)| team.message_bot(host, bot_id, &bot, &message),
            ),
            host_tool(
                self,
                &access,
                "share_file",
                "Shows a file from your own folders to the person, as a file card. When you're answering another bot, the file goes to that bot too. For Markdown images, explicitly include each companion's relative Markdown name and owned source path; no directory is searched.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "minLength": 1, "description": "An absolute path, or one relative to your working folder" },
                        "caption": { "type": "string", "maxLength": 500, "description": "A short note shown on the card" },
                        "companions": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string", "description": "The relative image name used in Markdown, such as images/chart.png" },
                                    "path": { "type": "string", "minLength": 1, "maxLength": 32767, "description": "An image source path, absolute or relative to your working folder" },
                                },
                                "required": ["name", "path"],
                                "additionalProperties": false,
                            },
                            "maxItems": MAX_ATTACHMENT_COMPANIONS,
                        },
                    },
                    "required": ["path"],
                }),
                parse_share_file,
                |team, host, bot_id, (path, caption, companions)| {
                    team.share_file(host, bot_id, &path, caption.as_deref(), &companions)
                },
            ),
            host_tool(
                self,
                &access,
                "add_routine",
                "Proposes a routine: work you do on a schedule. The person sees a card and decides. Nothing is saved until they approve it.",
                json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "minLength": 1, "maxLength": 60 },
                        "days": { "type": "array", "items": { "type": "string", "enum": DAYS }, "minItems": 1, "maxItems": 7, "description": "Days of the week" },
                        "times": { "type": "array", "items": { "type": "string", "pattern": "^([01]\\d|2[0-3]):[0-5]\\d$" }, "minItems": 1, "maxItems": 12, "description": "24-hour times in the person's time zone, like \"08:30\"" },
                        "prompt": { "type": "string", "minLength": 1, "maxLength": 10000, "description": "What to do on each run" },
                        "time_limit_minutes": { "type": "integer", "minimum": 1, "maximum": 1440, "description": "Stop each run after this many minutes" },
                    },
                    "required": ["name", "days", "times", "prompt"],
                }),
                parse_add_routine,
                |team, host, bot_id, proposal| team.propose_routine(host, bot_id, &proposal),
            ),
        ]
    }

    // ---- the tool bodies ----

    /// `request_login`: tells the person, in the chat the bot is writing to, which login to select. Reads nothing.
    pub fn request_login(
        &self,
        host: &mut dyn TeamHost,
        bot_id: &str,
        name: &str,
    ) -> Result<String, RpcError> {
        let bot = get_stored_bot(host.db(), bot_id)?;
        if bot.paused || !bot.browser {
            return Err(plain(
                "Resume this agent and enable its browser before requesting a login.",
            ));
        }
        let work = host.current_work(bot_id);
        self.add(
            host.db(),
            NewMessage {
                conversation_id: work.map_or_else(|| bot_id.to_owned(), |work| work.conversation_id),
                author: bot_id.to_owned(),
                kind: MessageKind::Text,
                body: format!("Login requested: {name}. Open Logins in this agent's Computer view to select a saved login and confirm the exact website on the host."),
                data: None,
                status: MessageStatus::Done,
            },
        )?;
        Ok("The person was asked to select a login. No credential was read or filled. Wait for their instructions before continuing browser automation.".to_owned())
    }

    /// `list_bots`: the bots on the caller's Can message list, with what they do and whether they're busy.
    pub fn list_bots(&self, host: &mut dyn TeamHost, bot_id: &str) -> Result<String, RpcError> {
        let targets = {
            let db = host.db();
            let ids = get_stored_bot(db, bot_id)?.can_message;
            ids.iter()
                .map(|id| get_stored_bot(db, id))
                .collect::<Result<Vec<_>, _>>()?
        };
        if targets.is_empty() {
            return Ok("You can't message any bots yet. The person can add bots to your Can message list in your settings.".to_owned());
        }
        let mut lines = Vec::new();
        for bot in targets {
            let description = first_line(&bot.instructions, 120);
            let name = bot.name.clone();
            let state = host.to_bot(bot).state;
            let description = if description.is_empty() {
                "No description.".to_owned()
            } else {
                description
            };
            lines.push(format!("- {name} ({}): {description}", state_word(state)));
        }
        Ok(lines.join("\n"))
    }

    /// `message_bot`: sends a request to another bot and returns at once. The exchange, its handoff rows and its work
    /// are written in one transaction; the answer comes back through [`Team::turn_ended`].
    pub fn message_bot(
        &self,
        host: &mut dyn TeamHost,
        from_id: &str,
        name: &str,
        request: &str,
    ) -> Result<String, RpcError> {
        let (from, to) = {
            let db = host.db();
            let from = get_stored_bot(db, from_id)?;
            let wanted = name.strip_prefix('@').unwrap_or(name).to_lowercase();
            let to = list_stored_bots(db)?
                .into_iter()
                .find(|bot| bot.name.to_lowercase() == wanted)
                .ok_or_else(|| {
                    plain(format!(
                        "There's no bot called {name}. Use list_bots to see who you can ask."
                    ))
                })?;
            (from, to)
        };
        if to.id == from.id {
            return Err(plain("You can't message yourself."));
        }
        if !from.can_message.contains(&to.id) {
            return Err(plain(format!(
                "You're not allowed to message {}. The person can add {} to your Can message list.",
                to.name, to.name
            )));
        }
        let work = host.current_work(from_id);
        let id = new_id();
        let queued = self.atomic(host, |host| {
            let db = host.db();
            let chain_id = match &work {
                Some(work) => match &work.chain_id {
                    Some(chain) => chain.clone(),
                    None => match get_work_item(db, &work.id)?.chain_id {
                        Some(chain) => chain,
                        None => new_chain(db)?,
                    },
                },
                None => {
                    let known = self.state().session_chains.get(from_id).cloned();
                    match known {
                        Some(chain) => chain,
                        None => new_chain(db)?,
                    }
                }
            };
            if !use_chain_request(db, &chain_id, i64::from(get_settings(db)?.chain_limit))? {
                return Err(plain("Limit reached, ask the user. This chain of requests between bots has used all its requests, so ask the person before asking another bot."));
            }
            if let Some(work) = &work
                && work.chain_id.is_none()
            {
                db.execute(
                    "UPDATE work_items SET chain_id = ?1 WHERE id = ?2",
                    params![chain_id, work.id],
                )?;
            }
            let reply_to = work
                .as_ref()
                .map_or_else(|| from.id.clone(), |work| work.conversation_id.clone());
            db.execute(
                "INSERT INTO exchanges (id, from_bot_id, to_bot_id, request, status, chain_id, reply_to, created_at, task_attempt_id, task_revision, execution_context_key) VALUES (?1, ?2, ?3, ?4, 'waiting', ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    id,
                    from.id,
                    to.id,
                    request,
                    chain_id,
                    reply_to,
                    now_iso(),
                    work.as_ref().and_then(|work| work.task_attempt_id.clone()),
                    work.as_ref().and_then(|work| work.task_revision),
                    work.as_ref().and_then(|work| work.execution_context_key.clone()),
                ],
            )?;
            db.execute(
                "INSERT INTO conversations (id, kind, bot_id) VALUES (?1, 'exchange', NULL)",
                [&id],
            )?;
            let message = self.add(
                db,
                NewMessage {
                    conversation_id: id.clone(),
                    author: from.id.clone(),
                    kind: MessageKind::Text,
                    body: request.to_owned(),
                    data: None,
                    status: MessageStatus::Done,
                },
            )?;
            for room_id in self.rooms_for(db, &from.id, &to.id)? {
                self.add(
                    db,
                    NewMessage {
                        conversation_id: room_id,
                        author: from.id.clone(),
                        kind: MessageKind::Text,
                        body: request.to_owned(),
                        data: Some(to_map(&ExchangePostData {
                            exchange_id: id.clone(),
                            to_bot_id: to.id.clone(),
                            files: None,
                        })),
                        status: MessageStatus::Done,
                    },
                )?;
            }
            let handoff = ExchangeHandoffData {
                exchange_id: id.clone(),
                from_bot_id: from.id.clone(),
                to_bot_id: to.id.clone(),
                messages: 1,
                status: ExchangeStatus::Waiting,
            };
            for bot_id in [&from.id, &to.id] {
                self.add(
                    db,
                    NewMessage {
                        conversation_id: bot_id.clone(),
                        author: from.id.clone(),
                        kind: MessageKind::Handoff,
                        body: first_line(request, 200),
                        data: Some(to_map(&handoff)),
                        status: MessageStatus::Done,
                    },
                )?;
            }
            enqueue(
                db,
                &NewWork {
                    message_id: Some(message.id.clone()),
                    conversation_id: Some(id.clone()),
                    chain_id: Some(chain_id.clone()),
                    exchange_id: Some(id.clone()),
                    ..NewWork::new(&to.id, WorkSource::BotRequest, &request_prompt(&from.name, request))
                },
            )
        })?;
        if work.is_none()
            && let Some(chain) = &queued.chain_id
        {
            self.state()
                .session_chains
                .insert(from_id.to_owned(), chain.clone());
        }
        if let Err(error) = self.wake(host, &queued) {
            self.atomic(host, |host| {
                let db = host.db();
                db.execute(
                    "UPDATE chains SET requests_made = requests_made - 1 WHERE id = ?1 AND requests_made > 0",
                    [&queued.chain_id],
                )?;
                let row = self
                    .exchange(db, &id)?
                    .ok_or_else(|| plain(format!("No exchange {id}")))?;
                self.fail(db, &row, "the request could not be delivered", None)
            })?;
            return Err(error);
        }
        Ok(format!("Sent. {} will reply.", to.name))
    }

    /// `share_file`: shows a file from the bot's own folders to the person, as a file card, from a bounded snapshot.
    /// While the bot is answering another bot, the file goes to that bot too.
    pub fn share_file(
        &self,
        host: &mut dyn TeamHost,
        bot_id: &str,
        path: &str,
        caption: Option<&str>,
        companions: &[CompanionArg],
    ) -> Result<String, RpcError> {
        let bot = get_stored_bot(host.db(), bot_id)?;
        let work = host.current_work(bot_id);
        let effective = host
            .effective_folder(bot_id)?
            .or_else(|| bot.project_folder.clone())
            .unwrap_or_else(|| bot.home_folder.clone());
        let roots = [PathBuf::from(&effective), PathBuf::from(&bot.home_folder)];
        let staging = &self.deps.shared_files;
        let canonical =
            |path: &Path| fs::canonicalize(path).map_err(|error| plain(error.to_string()));
        let managed = canonical(staging.parent().unwrap_or(staging))?;
        let on_a_task = work
            .as_ref()
            .is_some_and(|work| work.task_attempt_id.is_some());
        let checked = |relative: &str| -> Result<PathBuf, RpcError> {
            let file = file_inside(&roots, &roots[0].join(relative)).map_err(plain)?;
            if within(&managed, &file) {
                let home = canonical(Path::new(&bot.home_folder))?;
                if !within(&home, &file) {
                    let task_folder =
                        on_a_task && within(&canonical(Path::new(&effective))?, &file);
                    if !task_folder {
                        return Err(plain("You can only share files from your own folders."));
                    }
                }
            }
            Ok(file)
        };
        let names: HashSet<&str> = companions
            .iter()
            .map(|companion| companion.name.as_str())
            .collect();
        if names.len() != companions.len() {
            return Err(plain("Companion names must be unique."));
        }
        let mut sources = vec![checked(path)?];
        for companion in companions {
            sources.push(checked(&companion.path)?);
        }
        if !companions.is_empty() && self.attachments.get().is_none() {
            return Err(plain("Shared companion images are unavailable."));
        }
        let exchange = match &work {
            Some(work) => self.exchange(host.db(), &work.conversation_id)?,
            None => None,
        };
        let needs_reply = match &exchange {
            Some(exchange) if exchange.to_bot_id == bot_id => self
                .bot_or_null(host.db(), &exchange.from_bot_id)?
                .is_some(),
            _ => false,
        };
        let conversation = work
            .as_ref()
            .map_or_else(|| bot_id.to_owned(), |work| work.conversation_id.clone());
        let mut snapshots: Vec<FileData> = Vec::new();
        let mut durable: Vec<PathBuf> = Vec::new();
        let attempt = (|| -> Result<(), RpcError> {
            for source in &sources {
                snapshots.push(snapshot_file(&roots, source, staging, bot_id).map_err(plain)?);
                if let Some(attachments) = self.attachments.get() {
                    let last = snapshots.last_mut().expect("a snapshot was just pushed");
                    let info = attachments.register_snapshot(
                        host.db(),
                        Path::new(&last.path),
                        &last.name,
                    )?;
                    last.attachment_id = Some(info.id);
                    last.content_type = Some(info.content_type);
                }
            }
            if !companions.is_empty() {
                let markdown = snapshots[0].content_type.as_deref() == Some("text/markdown");
                let images = snapshots[1..].iter().all(|snapshot| {
                    matches!(
                        snapshot.content_type.as_deref(),
                        Some("image/png" | "image/jpeg" | "image/webp" | "image/gif")
                    )
                });
                if !markdown || !images {
                    return Err(plain(
                        "Only a Markdown document and explicitly shared raster images can form a companion bundle.",
                    ));
                }
                let listed: Vec<AttachmentCompanion> = companions
                    .iter()
                    .enumerate()
                    .map(|(index, companion)| AttachmentCompanion {
                        name: companion.name.clone(),
                        attachment_id: snapshots[index + 1]
                            .attachment_id
                            .clone()
                            .unwrap_or_default(),
                        attachment_index: index as u64 + 1,
                    })
                    .collect();
                snapshots[0].companions = Some(
                    AttachmentCompanions::new(listed).map_err(|error| plain(error.to_string()))?,
                );
            }
            if !needs_reply {
                for snapshot in &snapshots {
                    let home = Path::new(&bot.home_folder);
                    let copied = copy_into_inbox(home, Path::new(&snapshot.path))?;
                    durable.push(home.join("inbox").join(copied));
                }
            }
            self.atomic(host, |host| {
                for (index, snapshot) in snapshots.iter().enumerate() {
                    let mut card = snapshot.clone();
                    if let Some(copy) = durable.get(index) {
                        card.path = copy.to_string_lossy().into_owned();
                    }
                    let body = if index == 0 {
                        caption
                            .map(js::trim)
                            .filter(|caption| !caption.is_empty())
                            .unwrap_or(&snapshot.name)
                            .to_owned()
                    } else {
                        snapshot.name.clone()
                    };
                    self.add(
                        host.db(),
                        NewMessage {
                            conversation_id: conversation.clone(),
                            author: bot_id.to_owned(),
                            kind: MessageKind::File,
                            body,
                            data: Some(to_map(&card)),
                            status: MessageStatus::Done,
                        },
                    )?;
                }
                Ok(())
            })
        })();
        if let Err(error) = attempt {
            for copy in &durable {
                let _ = fs::remove_file(copy);
            }
            for snapshot in &snapshots {
                if let Some(folder) = Path::new(&snapshot.path).parent() {
                    let _ = fs::remove_dir_all(folder);
                }
            }
            if let Some(attachments) = self.attachments.get()
                && let Err(cleanup) = attachments.cleanup(host.db())
            {
                self.log(
                    "Tjommie: shared attachment cleanup failed",
                    &cleanup.to_string(),
                );
            }
            return Err(error);
        }
        let shared: Vec<PathBuf> = snapshots
            .iter()
            .map(|snapshot| PathBuf::from(&snapshot.path))
            .collect();
        match &work {
            Some(work) if needs_reply => {
                self.state()
                    .shared
                    .entry(work.id.clone())
                    .or_default()
                    .extend(shared);
            }
            _ => self.release_snapshots(&shared),
        }
        let extra = companions.len();
        let with = if extra == 0 {
            String::new()
        } else {
            format!(
                " with {extra} companion image{}",
                if extra == 1 { "" } else { "s" }
            )
        };
        Ok(format!("Shared {}{with}.", snapshots[0].name))
    }

    /// `add_routine`: proposes a routine as a card the person answers. Nothing is saved until they approve it, and a bot
    /// may have five proposals waiting at once.
    pub fn propose_routine(
        &self,
        host: &mut dyn TeamHost,
        bot_id: &str,
        proposal: &RoutineProposal,
    ) -> Result<String, RpcError> {
        let bot = get_stored_bot(host.db(), bot_id)?;
        let mut times = proposal.times.clone();
        times.sort_unstable();
        times.dedup();
        let limit = proposal
            .time_limit_minutes
            .filter(|&minutes| minutes > 0)
            .map_or_else(String::new, |minutes| {
                format!(", stop after {}", duration(minutes))
            });
        let summary = format!(
            "Add a routine: {}, {} at {}{limit}.",
            proposal.name,
            describe_days(&proposal.days),
            times.join(", ")
        );
        let approval_id = new_id();
        let payload = RoutineInput {
            bot_id: bot_id.to_owned(),
            name: proposal.name.clone(),
            prompt: proposal.prompt.clone(),
            days: proposal.days.clone(),
            times: proposal.times.clone(),
            time_zone: Some(
                TimeZone::new(host_time_zone()).map_err(|error| plain(error.to_string()))?,
            ),
            time_limit_minutes: Some(proposal.time_limit_minutes),
            enabled: None,
        };
        self.atomic(host, |host| {
            let approval = {
                let db = host.db();
                let pending: i64 = db.query_row(
                    "SELECT COUNT(*) FROM approvals WHERE bot_id = ?1 AND kind = 'routine' AND status = 'pending'",
                    [bot_id],
                    |row| row.get(0),
                )?;
                if pending >= MAX_PENDING_ROUTINE_PROPOSALS {
                    return Err(plain(format!("You already have {MAX_PENDING_ROUTINE_PROPOSALS} routine proposals waiting for the person. Wait for them to answer before proposing another.")));
                }
                let message = self.add(
                    db,
                    NewMessage {
                        conversation_id: bot_id.to_owned(),
                        author: bot_id.to_owned(),
                        kind: MessageKind::Approval,
                        body: summary.clone(),
                        data: Some(to_map(&RoutineApprovalData {
                            approval_id: approval_id.clone(),
                            tool: "Routine".to_owned(),
                            detail: proposal.prompt.clone(),
                            status: ApprovalStatus::Pending,
                            kind: RoutineKind::Routine,
                        })),
                        status: MessageStatus::Done,
                    },
                )?;
                create_approval(
                    db,
                    &NewApproval {
                        id: approval_id.clone(),
                        bot_id: bot_id.to_owned(),
                        kind: ApprovalKind::Routine,
                        tool: "Routine".to_owned(),
                        summary: summary.clone(),
                        detail: proposal.prompt.clone(),
                        rule: None,
                        payload: Some(to_map(&payload)),
                        message_id: message.id,
                    },
                )?
            };
            self.emit(Event::ApprovalAdded(approval));
            self.emit(Event::Notify(protocol::wire::Notify::Approval {
                bot_id: bot_id.to_owned(),
                title: format!("{} is waiting for you", bot.name),
                body: summary.clone(),
                approval_id: Some(approval_id.clone()),
            }));
            let view = host.to_bot(bot.clone());
            self.emit(Event::BotUpdated(view));
            Ok(())
        })?;
        Ok(
            "Proposed. The person sees a card and decides; nothing is saved until they approve it."
                .to_owned(),
        )
    }
}
