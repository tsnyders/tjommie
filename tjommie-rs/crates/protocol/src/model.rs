//! `model.ts`

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::attachments::AttachmentCompanions;
use crate::remote::RelayUrl;
use crate::{Invalid, Wire, parse};

/// The conversation id of the Team room, the one room every bot is in.
pub const TEAM_ROOM_ID: &str = "team";
pub const SETUP_SIGN_IN_PREFIX: &str = "Sign in to";
pub const SETUP_CODEX_SANDBOX_REASON: &str = "Set up Codex's sandbox";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

/// ask: every change needs you. folder: free inside its own folders. full: never asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Autonomy {
    Ask,
    Folder,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BotColor {
    Brown,
    Red,
    Orange,
    Amber,
    Green,
    Teal,
    Blue,
    Purple,
    Pink,
    Gray,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BotShape {
    Circle,
    Blob,
    RoundedSquare,
    Pill,
    Triangle,
    Hexagon,
    Cloud,
    Droplet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotState {
    Idle,
    Working,
    Waiting,
    Paused,
    Limited,
    NeedsSetup,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bot {
    pub id: String,
    pub name: String,
    pub color: BotColor,
    pub shape: BotShape,
    pub instructions: String,
    pub provider: Provider,
    pub model: String,
    /// Older v1 snapshots and fixtures omit the pin.
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub account_id: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub effective_account_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub using_fallback: Option<bool>,
    pub autonomy: Autonomy,
    pub home_folder: String,
    pub project_folder: Option<String>,
    pub browser: bool,
    pub paused: bool,
    pub can_message: Vec<String>,
    pub state: BotState,
    pub setup_reason: Option<String>,
    pub limited_until: Option<String>,
    pub created_at: String,
}

/// `bots.create` params. Omitted fields take the daemon's defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotInput {
    pub name: String,
    pub color: BotColor,
    pub shape: BotShape,
    pub instructions: String,
    pub provider: Provider,
    pub model: String,
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub account_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<Autonomy>,
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_folder: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_message: Option<Vec<String>>,
}

/// Fields left out stay unchanged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<BotColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<BotShape>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub account_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<Autonomy>,
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub project_folder: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_message: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageKind {
    Text,
    Activity,
    Approval,
    Notice,
    Error,
    Handoff,
    Routine,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageStatus {
    Queued,
    Sent,
    Streaming,
    Done,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    /// "you", "system" or a bot id.
    pub author: String,
    pub kind: MessageKind,
    pub body: String,
    /// Kept as the wire sends it so older and newer payloads survive; read it with [`message_data`].
    pub data: Option<Map<String, Value>>,
    pub status: MessageStatus,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Once,
    Always,
    Deny,
}

/// tool: a CLI asking to use a tool. routine: a bot proposing a routine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalKind {
    Tool,
    Routine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalStatus {
    Pending,
    Allowed,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: String,
    pub bot_id: String,
    pub kind: ApprovalKind,
    pub tool: String,
    pub summary: String,
    pub detail: String,
    pub rule: Option<String>,
    /// For a routine proposal, the `routines.create` params.
    pub payload: Option<Map<String, Value>>,
    pub status: ApprovalStatus,
    pub message_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupFields {
    pub installed: bool,
    pub version: Option<String>,
    pub signed_in: bool,
    pub account: Option<String>,
    pub plan: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexSandboxReadiness {
    Ready,
    NotConfigured,
    UpdateRequired,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexSetup {
    #[serde(flatten)]
    pub setup: SetupFields,
    pub sandbox: CodexSandboxReadiness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "lowercase")]
pub enum ProviderSetup {
    Claude(SetupFields),
    Codex(CodexSetup),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub is_default: bool,
}

static OFFSET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[+-](?:[01][0-9]|2[0-3])(?::?[0-5][0-9])?$").unwrap());

/// What `new Intl.DateTimeFormat("en", { timeZone })` accepts: IANA names in any case, and `±HH`, `±HHMM` or `±HH:MM`.
fn time_zone(value: &str) -> Option<String> {
    (OFFSET.is_match(value) || jiff::tz::TimeZone::get(value).is_ok()).then(|| value.to_owned())
}

refined_string!(
    /// An IANA time zone or UTC offset.
    TimeZone, time_zone, "not an IANA time zone"
);

impl TimeZone {
    /// A zone read back from storage, kept as it was saved. It was checked when it was saved (by Node's `Intl`, for a
    /// database an older daemon wrote), so it isn't checked again: a zone this build's database can't resolve must not make
    /// its row unreadable, as it must not in TypeScript. Resolving it fails where it is used, which is how a routine that
    /// can't be scheduled shows up. Anything that takes a zone from a person or the wire still goes through [`TimeZone::new`].
    pub fn from_stored(value: impl Into<String>) -> Self {
        TimeZone(value.into())
    }
}

/// Days are 0 (Sunday) to 6 (Saturday). Times are 24-hour "HH:MM" wall-clock times in the routine's time zone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineInput {
    pub bot_id: String,
    pub name: String,
    pub prompt: String,
    pub days: Vec<u8>,
    pub times: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<TimeZone>,
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub time_limit_minutes: Option<Option<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutinePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub times: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<TimeZone>,
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub time_limit_minutes: Option<Option<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Routine {
    pub id: String,
    pub bot_id: String,
    pub name: String,
    pub prompt: String,
    pub days: Vec<u8>,
    pub times: Vec<String>,
    pub time_zone: TimeZone,
    pub time_limit_minutes: Option<u32>,
    pub enabled: bool,
    pub next_run_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineRunStatus {
    Queued,
    Running,
    Done,
    Stopped,
    Failed,
    Missed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineRun {
    pub id: String,
    pub bot_id: String,
    pub routine_id: String,
    pub due_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub status: RoutineRunStatus,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExchangeStatus {
    Waiting,
    Replied,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Exchange {
    pub id: String,
    pub from_bot_id: String,
    pub to_bot_id: String,
    pub request: String,
    pub reply: Option<String>,
    pub status: ExchangeStatus,
    pub created_at: String,
    pub replied_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub id: String,
    pub name: String,
    pub bot_ids: Vec<String>,
    pub everyone: bool,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomInput {
    pub name: String,
    pub bot_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_ids: Option<Vec<String>>,
}

// Message.data payloads, by kind. They are separate from Message.data so wire readers keep accepting any record;
// message_data validates a payload before a typed reader uses it.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeHandoffData {
    pub exchange_id: String,
    pub from_bot_id: String,
    pub to_bot_id: String,
    pub messages: u64,
    pub status: ExchangeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomHandoffData {
    pub room_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HandoffData {
    Exchange(ExchangeHandoffData),
    Room(RoomHandoffData),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineLabelData {
    pub routine_id: String,
    pub run_id: String,
    pub status: RoutineRunStatus,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileData {
    pub path: String,
    pub name: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub companions: Option<AttachmentCompanions>,
}

/// The literal `"tool"`; older tool cards omit it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Tool,
}

/// The literal `"routine"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineKind {
    Routine,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolApprovalData {
    pub approval_id: String,
    pub tool: String,
    pub detail: String,
    pub status: ApprovalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ToolKind>,
    pub rule: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutineApprovalData {
    pub approval_id: String,
    pub tool: String,
    pub detail: String,
    pub status: ApprovalStatus,
    pub kind: RoutineKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ApprovalData {
    Tool(ToolApprovalData),
    Routine(RoutineApprovalData),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivityStatus {
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityItem {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub status: ActivityStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityData {
    pub items: Vec<ActivityItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorData {
    pub retry_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_bot_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoticeData {
    pub exchange_id: String,
}

/// Your attachments: inbox names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessageData {
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeFile {
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub companions: Option<AttachmentCompanions>,
}

/// An exchange post: files are the recipient's paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangePostData {
    pub exchange_id: String,
    pub to_bot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<ExchangeFile>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TextData {
    User(UserMessageData),
    ExchangePost(ExchangePostData),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "lowercase")]
pub enum MessageData {
    Text(Option<TextData>),
    Activity(ActivityData),
    Approval(ApprovalData),
    Notice(Option<NoticeData>),
    Error(Option<ErrorData>),
    Handoff(HandoffData),
    Routine(RoutineLabelData),
    File(FileData),
}

impl Wire for MessageData {
    const NAME: &'static str = "MessageData";
}

/// `messageData`: a message's payload, checked against its kind.
pub fn message_data(message: &Message) -> Result<MessageData, Invalid> {
    parse(json!({ "kind": message.kind, "data": message.data }))
}

/// chainLimit: how many bot-to-bot requests one chain (a message from you, a room post or a routine run) may make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub chain_limit: u32,
    pub relay_url: Option<RelayUrl>,
}

/// `settings.set` params: `Settings.partial()`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_limit: Option<u32>,
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub relay_url: Option<Option<RelayUrl>>,
}
