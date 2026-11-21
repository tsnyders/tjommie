//! The composer (Phase 3 plan T4.3; chat-composer.html; `composer.tsx`, `dictation-button.tsx`, `use-draft.ts`): the
//! large white card pinned under the chat. A hint says what the agent is doing to your message; the field takes it at
//! the Aegis body size; one row of round controls attaches files, mentions an agent, shows the agent's model (a menu
//! changes it), dictates, and sends. Enter sends at once and Shift+Enter starts a line. A working agent gets your
//! message as steering, or queues it; with the field empty the round button stops the turn instead, and only a press
//! stops it. Dictated words land in the draft for you to edit; dictation never sends, and while it runs a line says
//! what it's doing, with a way to cancel it.
//!
//! The draft is kept across a send (`use-draft.ts`): the send carries what was in the box when it was pressed, its
//! success clears the text only if nothing was typed since, and only the files that went. One send at a time; the field
//! is never disabled meanwhile.

use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, ExternalPaths, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px, svg,
};
use gpui_kit::base::input::{InputEditorStyle, InputEvent, Textarea, TextareaState};
use protocol::model::{
    ApprovalData, ApprovalStatus, Bot, BotPatch, BotState, Message, MessageData, MessageKind,
    ModelInfo, Provider, message_data,
};
use protocol::speech::{SpeechEvent, SpeechPhase};
use protocol::wire::{BotsUpdate, ChatSend, ChatStop, ProviderParams, method};

use super::voice;
use crate::guard::{guarded, guarded_async};
use crate::keymap::{self, MentionClose, MentionDown, MentionPick, MentionUp, Send};
use crate::model::attachments::{MAX_FILES, add_files, file_name};
use crate::model::dictation::{
    DraftEdit, InsertionRange, TRANSCRIPT_ADDED, TRANSCRIPT_MOVED, insert_transcript, move_range,
};
use crate::model::labels::{error_text, model_label};
use crate::model::mentions::{insert_mention, mention_options, typed_mention};
use crate::model::state::Connection;
use crate::motion::{clock, reduced, spring::Spring};
use crate::semantic::{SemanticExt as _, SemanticFocus as _};
use crate::theme::TextStyle;
use crate::ui::fields::Link;
use crate::ui::primitives::avatar::{BotAvatar, Pose};
use crate::ui::primitives::button::{IconButton, IconVariant};
use crate::ui::primitives::focus_handle;
use crate::ui::primitives::menu::{Menu, MenuItem};
use crate::ui::store;
use crate::ui::style::{TypeExt as _, hsla, palette, r, ring_or_reveal, shadows, theme};

/// Whether the agent has a turn to stop: it's working, or waiting on a tool card in its chat, which holds its turn. An
/// agent waiting only on a routine it proposed isn't running anything.
pub fn can_stop(state: BotState, messages: &[Message]) -> bool {
    match state {
        BotState::Working => true,
        BotState::Waiting => messages.iter().any(holds_turn),
        _ => false,
    }
}

fn holds_turn(message: &Message) -> bool {
    message.kind == MessageKind::Approval
        && matches!(
            message_data(message),
            Ok(MessageData::Approval(ApprovalData::Tool(tool))) if tool.status == ApprovalStatus::Pending
        )
}

/// The field's type: the Aegis body size, for the text and the placeholder alike (gpui-kit's input draws its
/// placeholder in the field's own text style).
pub fn field_type(cx: &App) -> &TextStyle {
    &theme(cx).typography.desktop.body
}

/// The edit that turned `before` into `after`, in UTF-16 units: the span between what they share at each end.
fn draft_edit(before: &str, after: &str) -> DraftEdit {
    let (a, b): (Vec<u16>, Vec<u16>) = (
        before.encode_utf16().collect(),
        after.encode_utf16().collect(),
    );
    let start = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let most = a.len().min(b.len()) - start;
    let same_tail = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(most)
        .take_while(|(x, y)| x == y)
        .count();
    DraftEdit {
        before: before.to_owned(),
        start,
        end: a.len() - same_tail,
    }
}

/// The byte offset of UTF-16 unit `unit` in `text`, or its end.
fn byte_of(text: &str, unit: usize) -> usize {
    let mut seen = 0;
    for (index, character) in text.char_indices() {
        if seen >= unit {
            return index;
        }
        seen += character.len_utf16();
    }
    text.len()
}

/// A file to send, by its path on this PC. Its chip shows the name only, as the TypeScript's did: nothing is read from
/// the disk on the UI thread (M4 review A3).
#[derive(Clone)]
struct Attachment {
    id: u64,
    path: String,
}

/// The "@..." being typed, and the highlighted completion.
struct Mention {
    start: usize,
    query: String,
    highlighted: usize,
}

/// A dictation in progress: its session, and where its words will go in the draft (`None` once an edit overlapped it).
struct Dictating {
    session: String,
    range: Option<InsertionRange>,
    phase: Option<SpeechPhase>,
}

pub struct Composer {
    bot_id: SharedString,
    field: Entity<TextareaState>,
    /// The agent's name as the placeholder says it ("Message <name>"), which follows a rename.
    named: String,
    /// The draft as of its last change, to move a dictation's range by each edit.
    text: String,
    files: Vec<Attachment>,
    next_file: u64,
    /// Goes up with every edit, so a send clears only a draft that's unchanged since.
    revision: u64,
    sending: bool,
    stopping: bool,
    problem: Option<SharedString>,
    mention: Option<Mention>,
    /// The agent's models, and the PC (`None`: this one) and provider they were asked of: a list is never shown for
    /// another PC or provider.
    models: Option<Vec<ModelInfo>>,
    models_for: Option<(Option<String>, Provider)>,
    /// Whether the connection was Ready when last drawn: a list that never came is asked for again when it's back.
    ready: bool,
    dictation: Option<Dictating>,
    /// The dictation the runtime last ended: why it ended can come after the end (a microphone that wouldn't open).
    ended: Option<String>,
    /// What dictation last said ("Transcript added to your draft. ..."), and a transcript that lost its place.
    voice_note: Option<SharedString>,
    recovered: Option<String>,
    focused: bool,
    lift: Spring,
    _subscriptions: Vec<Subscription>,
}

impl Composer {
    pub fn new(bot: &Bot, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = format!("Message {}", bot.name);
        let editor = editor_style(cx);
        let field = cx.new(|cx| {
            let mut field = TextareaState::new(window, cx)
                .placeholder(placeholder)
                .submit_on_enter(true)
                .auto_grow(1, 8);
            field.set_editor_style(editor);
            field
        });
        let voice = voice::voice(cx);
        let subscriptions = vec![
            cx.subscribe_in(&field, window, |this, _, event: &InputEvent, window, cx| {
                guarded("follow the draft", || this.follow_input(event, window, cx));
            }),
            cx.subscribe_in(&voice, window, |this, _, event, window, cx| {
                guarded("follow dictation", || this.follow_speech(event, window, cx));
            }),
        ];
        // Leaving the chat ends its dictation: the microphone never listens for a chat that's gone.
        cx.on_release(|this, cx| {
            guarded("end a dictation with its chat", || {
                this.cancel_dictation(cx)
            });
        })
        .detach();
        let calm = &theme(cx).motion.spring.calm;
        Composer {
            bot_id: SharedString::from(bot.id.clone()),
            field,
            named: bot.name.clone(),
            text: String::new(),
            files: Vec::new(),
            next_file: 0,
            revision: 0,
            sending: false,
            stopping: false,
            problem: None,
            mention: None,
            models: None,
            models_for: None,
            ready: false,
            dictation: None,
            ended: None,
            voice_note: None,
            recovered: None,
            focused: false,
            lift: Spring::new(0., calm),
            _subscriptions: subscriptions,
        }
    }

    /// Asks for the agent's models once the connection is Ready, and again when it comes back if the list never came.
    /// The list is for one PC and provider: on another PC's chat, or after a provider change, it's asked for afresh
    /// and the old one isn't shown meanwhile (M4 review A2).
    fn list_models(&mut self, provider: Provider, cx: &mut Context<Self>) {
        let state = store::state(cx);
        let ready = state.connection == Connection::Ready;
        let current = self
            .models_for
            .as_ref()
            .is_some_and(|(host, listed)| *host == state.host && *listed == provider);
        if !current {
            self.models = None;
        }
        let ask = ready && (!current || (self.models.is_none() && !self.ready));
        self.ready = ready;
        if !ask {
            return;
        }
        let wanted = (state.host.clone(), provider);
        self.models_for = Some(wanted.clone());
        let call = store::request::<method::ModelsList>(cx, ProviderParams { provider });
        cx.spawn(async move |this, cx| {
            guarded_async("list the models", async move {
                if let Ok(models) = call.await {
                    let _ = this.update(cx, |this, cx| {
                        // A list for a PC or provider no longer shown is dropped.
                        if this.models_for.as_ref() == Some(&wanted) {
                            this.models = Some(models);
                            cx.notify();
                        }
                    });
                }
            })
            .await;
        })
        .detach();
    }

    /// The placeholder follows a rename from another device, as the field's name does (exchange.rs does the same).
    fn follow_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.named == name {
            return;
        }
        self.named = name.to_owned();
        let placeholder = format!("Message {name}");
        self.field.update(cx, |field, cx| {
            field.set_placeholder(placeholder, window, cx)
        });
    }

    fn draft(&self, cx: &App) -> String {
        self.field.read(cx).value().to_string()
    }

    fn follow_input(&mut self, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            InputEvent::Change => {
                self.revision += 1;
                let text = self.draft(cx);
                if let Some(dictation) = &mut self.dictation {
                    let edit = draft_edit(&self.text, &text);
                    dictation.range = move_range(dictation.range, Some(&edit), &self.text, &text);
                }
                self.text = text;
                self.follow_mention(cx);
                cx.notify();
            }
            InputEvent::PressEnter {
                secondary: false,
                shift: false,
            } => self.send(window, cx),
            InputEvent::Focus => {
                self.focused = true;
                cx.notify();
            }
            InputEvent::Blur => {
                self.focused = false;
                self.mention = None;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {}
        }
    }

    /// Opens, narrows or closes the mention list for the "@..." before the caret. Your own agent and @everyone (a
    /// room's) aren't offered here.
    fn follow_mention(&mut self, cx: &App) {
        let caret = self.field.read(cx).cursor();
        let typed = typed_mention(&self.text, caret);
        self.mention = typed.and_then(|typed| {
            let count = self.options(&typed.query, cx).len();
            (count > 0).then(|| Mention {
                start: typed.start,
                highlighted: self
                    .mention
                    .as_ref()
                    .map_or(0, |open| open.highlighted.min(count - 1)),
                query: typed.query,
            })
        });
    }

    fn options(&self, query: &str, cx: &App) -> Vec<Bot> {
        let bots = &store::state(cx).bots;
        mention_options(query, bots)
            .into_iter()
            .filter_map(|option| option.bot.cloned())
            .filter(|bot| *bot.id != *self.bot_id)
            .collect()
    }

    fn move_mention(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(mention) = &self.mention else { return };
        let count = self.options(&mention.query, cx).len();
        if let Some(mention) = &mut self.mention
            && count > 0
        {
            mention.highlighted =
                (mention.highlighted as isize + step).rem_euclid(count as isize) as usize;
        }
        cx.notify();
    }

    /// Puts the highlighted agent's name in place of the "@..." being typed.
    fn pick_mention(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mention) = self.mention.take() else {
            return;
        };
        let Some(bot) = self
            .options(&mention.query, cx)
            .into_iter()
            .nth(mention.highlighted)
        else {
            return;
        };
        let caret = self.field.read(cx).cursor();
        if let Some(insertion) = insert_mention(&self.text, mention.start, caret, &bot.name) {
            self.field.update(cx, |field, cx| {
                field.set_value(insertion.text.clone(), window, cx);
                field.set_selected_range(insertion.caret..insertion.caret, cx);
            });
            self.revision += 1;
            self.text = insertion.text;
        }
        cx.notify();
    }

    /// The @ button: an "@" at the caret, which opens the list.
    fn start_mention(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let caret = self.field.read(cx).cursor();
        let needs_space = self
            .text
            .get(..caret)
            .and_then(|head| head.chars().next_back())
            .is_some_and(|previous| !previous.is_whitespace() && previous != '(');
        let at = if needs_space { " @" } else { "@" };
        self.field.update(cx, |field, cx| {
            field.insert(at, window, cx);
            field.focus(window, cx);
        });
    }

    /// Sends the draft (Enter or Send). A working agent takes it as steering, or queues it.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.draft(cx);
        if self.sending || text.trim().is_empty() {
            return;
        }
        self.sending = true;
        self.problem = None;
        self.mention = None;
        self.cancel_dictation(cx);
        // What this send carries, fixed now. Its success clears the text only if nothing was typed since, and only
        // these files. A file is a path on this PC, so another PC's chat sends none.
        let sent_revision = self.revision;
        let local = store::state(cx).host.is_none();
        let sent: Vec<Attachment> = if local {
            self.files.clone()
        } else {
            Vec::new()
        };
        let call = store::request::<method::ChatSend>(
            cx,
            ChatSend {
                bot_id: self.bot_id.to_string(),
                text,
                files: sent.iter().map(|file| file.path.clone()).collect(),
            },
        );
        let sent_ids: Vec<u64> = sent.iter().map(|file| file.id).collect();
        cx.spawn_in(window, async move |this, cx| {
            guarded_async("send a message", async move {
                let outcome = call.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.sending = false;
                    match outcome {
                        Ok(_) => {
                            if this.revision == sent_revision {
                                this.field
                                    .update(cx, |field, cx| field.set_value("", window, cx));
                                this.text.clear();
                            }
                            this.files.retain(|file| !sent_ids.contains(&file.id));
                        }
                        Err(error) => this.problem = Some(error_text(&error).into()),
                    }
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
        cx.notify();
    }

    /// Stops the agent's turn; stopping clears nothing.
    fn stop(&mut self, cx: &mut Context<Self>) {
        if self.stopping {
            return;
        }
        self.stopping = true;
        self.problem = None;
        let call = store::request::<method::ChatStop>(
            cx,
            ChatStop {
                bot_id: self.bot_id.to_string(),
            },
        );
        cx.spawn(async move |this, cx| {
            guarded_async("stop a turn", async move {
                let outcome = call.await;
                let _ = this.update(cx, |this, cx| {
                    this.stopping = false;
                    this.problem = outcome.err().map(|error| error_text(&error).into());
                    cx.notify();
                });
            })
            .await;
        })
        .detach();
        cx.notify();
    }

    /// Adds files, as the attachments are now, so a file a send cleared never comes back.
    fn attach(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let current: Vec<String> = self.files.iter().map(|file| file.path.clone()).collect();
        let incoming: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        match add_files(&current, &incoming) {
            Some(next) => {
                for path in next.into_iter().skip(current.len()) {
                    self.next_file += 1;
                    self.files.push(Attachment {
                        id: self.next_file,
                        path,
                    });
                }
                self.problem = None;
            }
            None => {
                self.problem = Some(format!("A message can carry up to {MAX_FILES} files.").into())
            }
        }
        cx.notify();
    }

    /// The + button: the file picker, on this PC.
    fn pick_files(&mut self, cx: &mut Context<Self>) {
        if store::state(cx).host.is_some() {
            self.problem = Some(ELSEWHERE.into());
            cx.notify();
            return;
        }
        let picked = crate::ui::prompt_for_paths(
            PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: None,
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            guarded_async("attach files", async move {
                if let Ok(Ok(Some(paths))) = picked.await {
                    let _ = this.update(cx, |this, cx| this.attach(paths, cx));
                }
            })
            .await;
        })
        .detach();
    }

    fn change_model(&mut self, model: String, cx: &mut Context<Self>) {
        let call = store::request::<method::BotsUpdate>(
            cx,
            BotsUpdate {
                id: self.bot_id.to_string(),
                patch: BotPatch {
                    model: Some(model),
                    ..BotPatch::default()
                },
            },
        );
        cx.spawn(async move |this, cx| {
            guarded_async("change the model", async move {
                if let Err(error) = call.await {
                    let _ = this.update(cx, |this, cx| {
                        this.problem = Some(error_text(&error).into());
                        cx.notify();
                    });
                }
            })
            .await;
        })
        .detach();
    }

    /// The mic: starts dictating where the caret is, or finishes; the words land in the draft.
    fn toggle_dictation(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = voice::backend(cx) else {
            self.voice_note = Some("Dictation isn't available on this PC yet.".into());
            cx.notify();
            return;
        };
        if let Some(dictation) = &self.dictation {
            backend.finish(&dictation.session, cx);
            return;
        }
        let selected = self.field.read(cx).selected_range();
        let units = |byte: usize| {
            self.text
                .get(..byte)
                .map_or(0, |head| head.encode_utf16().count())
        };
        let session = session_id();
        self.dictation = Some(Dictating {
            session: session.clone(),
            range: Some(InsertionRange {
                start: units(selected.start),
                end: units(selected.end),
            }),
            phase: None,
        });
        self.ended = None;
        self.voice_note = None;
        self.recovered = None;
        backend.begin(&session, cx);
        cx.notify();
    }

    fn cancel_dictation(&mut self, cx: &mut App) {
        if let Some(dictation) = self.dictation.take()
            && let Some(backend) = voice::backend(cx)
        {
            backend.cancel(&dictation.session, cx);
        }
    }

    fn follow_speech(&mut self, event: &SpeechEvent, window: &mut Window, cx: &mut Context<Self>) {
        let mine = |session: &str| {
            self.dictation
                .as_ref()
                .is_some_and(|d| d.session == session)
        };
        match event {
            SpeechEvent::Session {
                session_id,
                phase,
                problem,
            } if mine(session_id) => {
                if matches!(phase, SpeechPhase::Cancelled | SpeechPhase::Error) {
                    self.dictation = None;
                    self.ended = Some(session_id.clone());
                    self.voice_note = problem
                        .as_ref()
                        .map(|problem| problem.message.clone().into());
                } else if let Some(dictation) = &mut self.dictation {
                    dictation.phase = Some(*phase);
                }
                cx.notify();
            }
            // Why a dictation the runtime ended stopped, said after the end: a microphone that wouldn't open is
            // cancelled first, and its reason follows.
            SpeechEvent::Session {
                session_id,
                problem: Some(problem),
                ..
            } if self.ended.as_deref() == Some(session_id.as_str()) => {
                self.voice_note = Some(problem.message.clone().into());
                cx.notify();
            }
            SpeechEvent::Final { session_id, text } if mine(session_id) => {
                let range = self.dictation.take().and_then(|dictation| dictation.range);
                let draft = self.draft(cx);
                match range.map(|range| insert_transcript(&draft, range, text)) {
                    Some(Ok(insertion)) => {
                        let caret = byte_of(&insertion.text, insertion.caret);
                        self.field.update(cx, |field, cx| {
                            field.set_value(insertion.text.clone(), window, cx);
                            field.set_selected_range(caret..caret, cx);
                        });
                        self.revision += 1;
                        self.text = insertion.text;
                        self.voice_note = Some(TRANSCRIPT_ADDED.into());
                    }
                    _ => {
                        self.recovered = Some(text.clone());
                        self.voice_note = Some(TRANSCRIPT_MOVED.into());
                    }
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// Puts a transcript that lost its place at the caret.
    fn insert_recovered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.recovered.take() {
            self.field.update(cx, |field, cx| {
                field.insert(text, window, cx);
                field.focus(window, cx);
            });
            self.voice_note = Some(TRANSCRIPT_ADDED.into());
            cx.notify();
        }
    }
}

const ELSEWHERE: &str = "Attach files in Tjommie on that PC.";

/// A new dictation session's id.
fn session_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "dictation-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn editor_style(cx: &App) -> InputEditorStyle {
    let colors = palette(cx);
    InputEditorStyle {
        foreground: hsla(colors.text.primary),
        muted_foreground: hsla(colors.text.tertiary),
        selection: hsla(colors.state.pressed),
        caret: hsla(colors.text.primary),
        ..InputEditorStyle::default()
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(bot) = store::state(cx)
            .bots
            .iter()
            .find(|bot| *bot.id == *self.bot_id)
            .cloned()
        else {
            return div().into_any_element();
        };
        self.list_models(bot.provider, cx);
        self.follow_name(&bot.name, window, cx);
        let state = store::state(cx);
        let messages = state
            .conversations
            .get(self.bot_id.as_ref())
            .map(|conversation| conversation.messages.as_slice())
            .unwrap_or_default();
        let has_text = !self.text.trim().is_empty();
        let stop = !has_text && can_stop(bot.state, messages);
        let waiting_on_you = bot.state == BotState::Waiting && messages.iter().any(holds_turn);
        let local = state.host.is_none();
        let options = self
            .mention
            .as_ref()
            .map(|mention| self.options(&mention.query, cx))
            .unwrap_or_default();

        // The card lifts 2 px while the field has the focus (Aegis composer focus), on the calm spring.
        let lift = if self.focused {
            theme(cx).motion.distance.lift
        } else {
            0.
        };
        if reduced::is_on(cx) {
            self.lift.snap(lift);
        } else {
            self.lift.set_target(lift);
            if self.lift.step(clock::dt(cx)) {
                clock::keep_moving(window, cx);
            }
        }

        // The model pill is a Tab stop, as every menu's trigger is: Enter or Space opens its menu.
        let model_focus = focus_handle(&format!("composer.{}.model.open", self.bot_id), window, cx);
        // The field's handle is kept under its id, so an answered approval can hand the keys back to it (A4).
        let field_id = format!("composer.{}.field", self.bot_id);
        let field_focus = self.field.focus_handle(cx);
        crate::ui::primitives::remember_focus(&field_id, &field_focus, cx);
        let theme = theme(cx);
        let colors = palette(cx);
        let id = self.bot_id.clone();
        let entity = cx.entity().downgrade();

        let hint = if waiting_on_you {
            Some((true, format!("{} is waiting for your approval", bot.name)))
        } else if bot.state == BotState::Working {
            Some((
                false,
                format!(
                    "{} is working. Your message joins this turn, or queues if it can't.",
                    bot.name
                ),
            ))
        } else {
            None
        };

        let chips: Vec<AnyElement> = if local {
            self.files
                .iter()
                .map(|file| {
                    let name = SharedString::from(file_name(&file.path).to_owned());
                    let remove = {
                        let (entity, file_id) = (entity.clone(), file.id);
                        IconButton::new(
                            format!("composer.{id}.file.{}.remove", file.id),
                            "x",
                            format!("Remove {name}"),
                        )
                        .on_press(move |_, cx| {
                            let _ = entity.update(cx, |this, cx| {
                                this.files.retain(|file| file.id != file_id);
                                cx.notify();
                            });
                        })
                    };
                    div()
                        .semantic(format!("composer.{id}.file.{}", file.id), Role::ListItem)
                        .aria_label(name.clone())
                        .h(r(36.))
                        .max_w(r(280.))
                        .flex()
                        .items_center()
                        .gap(r(theme.space.s8))
                        .pl(r(theme.space.s12))
                        .pr(r(theme.space.s4))
                        .rounded(r(theme.radius.field))
                        .bg(hsla(colors.surface.panel))
                        .type_style(&theme.typography.desktop.small)
                        .text_color(hsla(colors.text.primary))
                        .child(
                            svg()
                                .path("icons/i/175/file-text.svg")
                                .size(r(15.))
                                .flex_none()
                                .text_color(hsla(colors.icon.base)),
                        )
                        .child(div().min_w_0().truncate().child(name))
                        .child(remove)
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };

        let mention_list = self
            .mention
            .as_ref()
            .filter(|_| !options.is_empty())
            .map(|mention| {
                // A listbox the field owns, as the TypeScript's was (M4 review C2): the arrows move the selected
                // option while the field keeps the keyboard.
                let rows = options.iter().enumerate().map(|(index, option)| {
                    let highlighted = index == mention.highlighted;
                    let pick = entity.clone();
                    div()
                        .semantic(
                            format!("composer.{id}.mention.{}", option.id),
                            Role::ListBoxOption,
                        )
                        .aria_label(SharedString::from(option.name.clone()))
                        .aria_selected(highlighted)
                        // The screen reader's current option. gpui honours the claim only while an ancestor holds
                        // the focus, and the field's input is a sibling of this list, so for now it is a no-op.
                        .when(highlighted, |row| row.aria_active_descendant())
                        .h(r(36.))
                        .flex()
                        .items_center()
                        .gap(r(theme.space.s10))
                        .px(r(theme.space.s10))
                        .rounded(r(theme.radius.sm))
                        .cursor_pointer()
                        .when(highlighted, |row| row.bg(hsla(colors.state.hover)))
                        .type_style(&theme.typography.desktop.ui)
                        .text_color(hsla(colors.text.primary))
                        .on_click(move |_, window, cx| {
                            guarded("pick a mention", || {
                                let _ = pick.update(cx, |this, cx| {
                                    if let Some(mention) = &mut this.mention {
                                        mention.highlighted = index;
                                    }
                                    this.pick_mention(window, cx);
                                });
                            });
                        })
                        .child(
                            BotAvatar::new(option.color, option.shape, theme.size.avatar20)
                                .pose(Pose::of(option.state)),
                        )
                        .child(SharedString::from(option.name.clone()))
                });
                div()
                    .semantic(format!("composer.{id}.mentions"), Role::ListBox)
                    .aria_label("Mention an agent")
                    .absolute()
                    .bottom_full()
                    .left(r(theme.space.s12))
                    .mb(r(theme.space.s8))
                    .min_w(r(240.))
                    .p(r(theme.space.s6))
                    .bg(hsla(colors.surface.raised))
                    .rounded(r(theme.radius.md))
                    .border_t_1()
                    .border_color(hsla(colors.overlay.highlight))
                    .shadow(shadows(&theme.shadow.light.raised))
                    .children(rows)
            });
        let listing = mention_list.is_some();

        let (provider, model) = {
            let label = model_label(bot.provider, &bot.model);
            match label.split_once(" · ") {
                Some((provider, model)) => (provider.to_owned(), model.to_owned()),
                None => (label, String::new()),
            }
        };
        let model_pill = {
            let trigger_id = format!("composer.{id}.model.open");
            let (provider, model) = (SharedString::from(provider), SharedString::from(model));
            let pill = move |open: bool, window: &Window, cx: &App| {
                let theme = crate::ui::style::theme(cx);
                let colors = palette(cx);
                div()
                    .semantic(trigger_id.clone(), Role::Button)
                    .aria_label(SharedString::from(format!("Model: {provider} {model}")))
                    .aria_expanded(open)
                    .track_focus(&model_focus)
                    .relative()
                    .h(r(36.))
                    .px(r(theme.space.s14))
                    .flex()
                    .items_center()
                    .gap(r(theme.space.s8))
                    .rounded_full()
                    .cursor_pointer()
                    .bg(hsla(if open {
                        colors.state.panel_hover
                    } else {
                        colors.surface.panel
                    }))
                    .hover(|style| style.bg(hsla(colors.state.panel_hover)))
                    .type_style(&theme.typography.desktop.ui)
                    .text_color(hsla(colors.text.secondary))
                    .whitespace_nowrap()
                    .child(provider.clone())
                    .child(
                        div()
                            .font_weight(gpui::FontWeight(500.))
                            .text_color(hsla(colors.text.primary))
                            .child(model.clone()),
                    )
                    .children(ring_or_reveal(&model_focus, theme.radius.pill, window, cx))
                    .into_any_element()
            };
            let mut menu = Menu::new(SharedString::from(format!("composer.{id}.model")), pill)
                .anchor(gpui::Anchor::BottomLeft)
                .offset(px(8.));
            let models = self.models.clone().unwrap_or_default();
            if models.is_empty() {
                menu = menu.note("The other models show once this PC lists them.");
            }
            for model in models {
                let (pick, value) = (entity.clone(), model.id.clone());
                menu = menu.item(
                    MenuItem::new(
                        format!("composer.{id}.model.{}", model.id),
                        model.label.clone(),
                        move |_, cx| {
                            let _ =
                                pick.update(cx, |this, cx| this.change_model(value.clone(), cx));
                        },
                    )
                    .checked(model.id == bot.model),
                );
            }
            menu
        };

        // A file goes as its path on this PC, so on another PC's chat Attach is unavailable and its name says where to
        // attach instead (the TypeScript's aria-disabled with its reason).
        let attach = {
            let entity = entity.clone();
            let name = if local {
                "Attach files"
            } else {
                ELSEWHERE.trim_end_matches('.')
            };
            IconButton::new(format!("composer.{id}.attach"), "plus", name)
                .variant(IconVariant::Grey)
                .disabled(!local)
                .on_press(move |_, cx| {
                    let _ = entity.update(cx, |this, cx| this.pick_files(cx));
                })
        };
        let mention = {
            let entity = entity.clone();
            IconButton::new(
                format!("composer.{id}.mention"),
                "at-sign",
                "Mention an agent",
            )
            .variant(IconVariant::Grey)
            .on_press(move |window, cx| {
                let _ = entity.update(cx, |this, cx| this.start_mention(window, cx));
            })
        };
        let dictating = self.dictation.is_some();
        let mic = {
            let entity = entity.clone();
            IconButton::new(
                format!("composer.{id}.dictate"),
                "mic",
                if dictating {
                    "Stop dictating"
                } else {
                    "Start dictation"
                },
            )
            .variant(if dictating {
                IconVariant::Dark
            } else {
                IconVariant::Grey
            })
            .on_press(move |_, cx| {
                let _ = entity.update(cx, |this, cx| this.toggle_dictation(cx));
            })
        };
        // Stop and Send are one button under one id, so a focused Stop that turns into Send keeps the focus (M4 review
        // A4). Send with nothing to send is unavailable: no Tab stop, though a focused one keeps the focus.
        let round = {
            let entity = entity.clone();
            let (icon, name): (&'static str, SharedString) = if stop {
                ("square", format!("Stop {}", bot.name).into())
            } else {
                ("arrow-up", "Send".into())
            };
            IconButton::new(format!("composer.{id}.round"), icon, name)
                .variant(if stop || has_text {
                    IconVariant::Dark
                } else {
                    IconVariant::Grey
                })
                .disabled(!stop && !has_text)
                .on_press(move |window, cx| {
                    let _ = entity.update(cx, |this, cx| {
                        if stop {
                            this.stop(cx);
                        } else {
                            this.send(window, cx);
                        }
                    });
                })
        };

        // While dictating: what it's doing, and a way out (the TypeScript's `DictationStatus`).
        let dictation_line = self.dictation.as_ref().map(|dictation| {
            let doing = match dictation.phase {
                Some(SpeechPhase::Listening) => "Listening…",
                Some(SpeechPhase::Transcribing) => "Transcribing…",
                _ => "Preparing voice…",
            };
            let cancel = entity.clone();
            div()
                .semantic(format!("composer.{id}.dictation"), Role::Status)
                .aria_label(doing)
                .flex()
                .items_center()
                .gap(r(theme.space.s12))
                .px(r(theme.space.s6))
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.text.secondary))
                .child(div().flex_1().min_w_0().child(doing))
                .child(
                    Link::new(
                        format!("composer.{id}.dictation.cancel"),
                        "Cancel dictation",
                    )
                    .quiet()
                    .on_press(move |window, cx| {
                        let _ = cancel.update(cx, |this, cx| {
                            this.cancel_dictation(cx);
                            // The line goes with the dictation, so the keyboard carries on from the field.
                            this.field.update(cx, |field, cx| field.focus(window, cx));
                            cx.notify();
                        });
                    }),
                )
        });

        let voice_line = self.voice_note.clone().map(|note| {
            let recovered = self.recovered.is_some();
            let (insert, discard) = (entity.clone(), entity.clone());
            div()
                .semantic(format!("composer.{id}.voice"), Role::Status)
                .aria_label(note.clone())
                .flex()
                .items_center()
                .gap(r(theme.space.s12))
                .px(r(theme.space.s6))
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.text.secondary))
                .child(div().flex_1().min_w_0().child(note))
                .when(recovered, |line| {
                    line.child(
                        Link::new(format!("composer.{id}.voice.insert"), "Insert transcript")
                            .on_press(move |window, cx| {
                                let _ =
                                    insert.update(cx, |this, cx| this.insert_recovered(window, cx));
                            }),
                    )
                    .child(
                        Link::new(format!("composer.{id}.voice.discard"), "Discard")
                            .quiet()
                            .on_press(move |_, cx| {
                                let _ = discard.update(cx, |this, cx| {
                                    this.recovered = None;
                                    this.voice_note = None;
                                    cx.notify();
                                });
                            }),
                    )
                })
        });

        let body = field_type(cx).clone();
        // The field owns the mention list (a combo box, as the room composer's is) and reports its input's focus to
        // the test driver (M4 review C4).
        let field = div()
            .semantic(field_id, Role::ComboBox)
            .aria_label(SharedString::from(format!("Message {}", bot.name)))
            .aria_expanded(listing)
            .semantic_focus(&field_focus)
            .when(self.mention.is_some(), |field| {
                field.key_context(keymap::MENTIONS)
            })
            .px(r(theme.space.s6))
            .pt(r(theme.space.s6))
            .pb(r(theme.space.s10))
            .type_style(&body)
            .text_color(hsla(colors.text.primary))
            .child(Textarea::new(&self.field));

        let on_send = entity.clone();
        let (up, down, pick, close) = (
            entity.clone(),
            entity.clone(),
            entity.clone(),
            entity.clone(),
        );
        let drop = entity.clone();
        div()
            .key_context(keymap::COMPOSER)
            .on_action(move |_: &Send, window, cx| {
                guarded("send by Enter", || {
                    let _ = on_send.update(cx, |this, cx| this.send(window, cx));
                });
            })
            .on_action(move |_: &MentionUp, _, cx| {
                guarded("move the mention up", || {
                    let _ = up.update(cx, |this, cx| this.move_mention(-1, cx));
                });
            })
            .on_action(move |_: &MentionDown, _, cx| {
                guarded("move the mention down", || {
                    let _ = down.update(cx, |this, cx| this.move_mention(1, cx));
                });
            })
            .on_action(move |_: &MentionPick, window, cx| {
                guarded("pick the mention", || {
                    let _ = pick.update(cx, |this, cx| this.pick_mention(window, cx));
                });
            })
            .on_action(move |_: &MentionClose, _, cx| {
                guarded("close the mentions", || {
                    let _ = close.update(cx, |this, cx| {
                        this.mention = None;
                        cx.notify();
                    });
                });
            })
            .flex_none()
            .w_full()
            .px(r(theme.space.s16))
            .pb(r(theme.space.s16))
            .child(
                div()
                    .id(SharedString::from(format!("composer.{id}.card")))
                    .relative()
                    .top(px(-self.lift.value))
                    .w_full()
                    .flex()
                    .flex_col()
                    .px(r(theme.space.s12))
                    .pt(r(theme.space.s12))
                    .pb(r(theme.space.s10))
                    .bg(hsla(colors.surface.raised))
                    .rounded(r(theme.radius.card))
                    .border_t_1()
                    .border_color(hsla(colors.overlay.highlight))
                    .shadow(shadows(if self.focused {
                        &theme.shadow.light.raised_focus
                    } else {
                        &theme.shadow.light.raised
                    }))
                    // Files dropped on the card come in as attachments; on another PC's chat they can't.
                    .drag_over::<ExternalPaths>(|style, _, _, cx| {
                        style.bg(hsla(palette(cx).surface.panel))
                    })
                    .on_drop(move |paths: &ExternalPaths, _, cx| {
                        guarded("drop files", || {
                            let _ = drop.update(cx, |this, cx| {
                                if store::state(cx).host.is_some() {
                                    this.problem = Some(ELSEWHERE.into());
                                    cx.notify();
                                } else {
                                    this.attach(paths.paths().to_vec(), cx);
                                }
                            });
                        });
                    })
                    .children(mention_list)
                    .when_some(hint, |card, (dot, hint)| {
                        card.child(
                            div()
                                .semantic(format!("composer.{id}.hint"), Role::Status)
                                .aria_label(SharedString::from(hint.clone()))
                                .flex()
                                .items_center()
                                .gap(r(theme.space.s8))
                                .px(r(theme.space.s6))
                                .pt(r(theme.space.s2))
                                .type_style(&theme.typography.desktop.small)
                                .text_color(hsla(colors.text.secondary))
                                .when(dot, |hint| {
                                    hint.child(
                                        div()
                                            .size(r(8.))
                                            .flex_none()
                                            .rounded_full()
                                            .bg(hsla(colors.accent.marker)),
                                    )
                                })
                                .child(hint),
                        )
                    })
                    .when(!chips.is_empty(), |card| {
                        card.child(
                            div()
                                .semantic(format!("composer.{id}.files"), Role::List)
                                .aria_label("Attached files")
                                .flex()
                                .flex_wrap()
                                .gap(r(theme.space.s6))
                                .px(r(theme.space.s4))
                                .pt(r(theme.space.s4))
                                .children(chips),
                        )
                    })
                    .child(field)
                    .children(dictation_line)
                    .children(voice_line)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(r(theme.space.s8))
                            .child(attach)
                            .child(mention)
                            .child(model_pill)
                            .child(div().flex_1())
                            .child(mic)
                            .child(round),
                    ),
            )
            .when_some(self.problem.clone(), |composer, problem| {
                composer.child(
                    div()
                        .semantic(format!("composer.{id}.problem"), Role::Alert)
                        .aria_label(problem.clone())
                        .mt(r(theme.space.s6))
                        .px(r(theme.space.s12))
                        .type_style(&theme.typography.desktop.small)
                        .text_color(hsla(theme.color.bot.red))
                        .child(problem),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn card(id: &str, data: serde_json::Value) -> Message {
        let mut base =
            json!({ "approvalId": format!("a-{id}"), "tool": "Write", "detail": "hello.txt" });
        if let (Some(base), Some(extra)) = (base.as_object_mut(), data.as_object()) {
            base.extend(extra.clone());
        }
        Message {
            id: id.into(),
            conversation_id: "b1".into(),
            author: "b1".into(),
            kind: MessageKind::Approval,
            body: "Write hello.txt".into(),
            data: base.as_object().cloned(),
            status: protocol::model::MessageStatus::Done,
            created_at: "2026-09-27T07:14:00.000Z".into(),
        }
    }

    #[test]
    fn offers_stop_while_the_agent_works_or_waits_on_a_tool_card_that_holds_its_turn() {
        let tool = card("m1", json!({ "status": "pending", "rule": null }));
        let answered = card("m3", json!({ "status": "allowed", "rule": null }));
        let typed = card(
            "m4",
            json!({ "status": "pending", "rule": null, "kind": "tool" }),
        );
        assert!(can_stop(BotState::Working, &[]));
        assert!(can_stop(
            BotState::Waiting,
            &[answered.clone(), tool.clone()]
        ));
        assert!(can_stop(BotState::Waiting, &[typed]));
    }

    #[test]
    fn offers_no_stop_for_an_agent_that_only_waits_on_its_routine_or_isnt_running() {
        let tool = card("m1", json!({ "status": "pending", "rule": null }));
        let routine = card("m2", json!({ "status": "pending", "kind": "routine" }));
        let answered = card("m3", json!({ "status": "allowed", "rule": null }));
        assert!(!can_stop(BotState::Waiting, &[routine, answered]));
        assert!(!can_stop(BotState::Waiting, &[]));
        assert!(!can_stop(BotState::Idle, std::slice::from_ref(&tool)));
        assert!(!can_stop(BotState::Paused, &[tool]));
    }

    mod views {
        use std::cell::RefCell;
        use std::rc::Rc;

        use gpui::TestAppContext;
        use protocol::model::{BotState, ModelInfo};
        use protocol::speech::{SpeechEvent, SpeechPhase, SpeechProblem};
        use protocol::wire::method;

        use crate::model::dictation::TRANSCRIPT_ADDED;
        use crate::model::state::Connection;
        use crate::model::testing::FakeRpc;
        use crate::ui::chat::tests::{calls_to, chat, message, turns};
        use crate::ui::chat::voice::{self, VoiceBackend};
        use crate::ui::shell::route::Route;
        use crate::ui::store;
        use crate::ui::testing::{app, bot, click, draw, node, state, text};

        fn sends() -> FakeRpc {
            FakeRpc::new().on::<method::ChatSend>(|params| {
                Ok(message(
                    "sent",
                    "you",
                    params["text"].as_str().unwrap_or(""),
                    1,
                ))
            })
        }

        #[gpui::test]
        fn enter_sends_once(cx: &mut TestAppContext) {
            let (cx, calls) = app(cx, Route::Chat("b1".into()), chat(turns(2), false), sends());
            click(cx, "composer.b1.field");
            cx.simulate_input("Run the tests");
            cx.simulate_keystrokes("enter");
            draw(cx);
            // The draft went and was cleared, so a second Enter has nothing to send.
            cx.simulate_keystrokes("enter");
            draw(cx);
            let sent = calls_to(&calls, "chat.send");
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert_eq!(sent[0]["text"], "Run the tests");
            assert_eq!(sent[0]["botId"], "b1");
        }

        #[gpui::test]
        fn shift_enter_starts_a_line_and_sends_nothing(cx: &mut TestAppContext) {
            let (cx, calls) = app(cx, Route::Chat("b1".into()), chat(turns(2), false), sends());
            click(cx, "composer.b1.field");
            cx.simulate_input("First");
            cx.simulate_keystrokes("shift-enter");
            cx.simulate_input("second");
            draw(cx);
            assert!(calls_to(&calls, "chat.send").is_empty());
            cx.simulate_keystrokes("enter");
            draw(cx);
            assert_eq!(calls_to(&calls, "chat.send")[0]["text"], "First\nsecond");
        }

        #[gpui::test]
        fn the_draft_typed_while_a_send_is_on_its_way_stays(cx: &mut TestAppContext) {
            let (cx, calls) = app(cx, Route::Chat("b1".into()), chat(turns(2), false), sends());
            click(cx, "composer.b1.field");
            cx.simulate_input("One");
            // Enter, then more typing before the send's answer is applied: the edit isn't cleared with what went.
            cx.update(|window, cx| {
                window.dispatch_keystroke(gpui::Keystroke::parse("enter").unwrap(), cx);
            });
            cx.simulate_input(" more");
            draw(cx);
            cx.simulate_keystrokes("enter");
            draw(cx);
            let sent = calls_to(&calls, "chat.send");
            assert_eq!(sent.len(), 2, "{sent:?}");
            assert_eq!(
                sent[1]["text"], "One more",
                "the edited draft stayed, whole"
            );
        }

        /// A dictation backend that records what it was asked.
        #[derive(Default)]
        struct FakeVoice(RefCell<Vec<String>>);

        impl VoiceBackend for FakeVoice {
            fn begin(&self, session_id: &str, _: &mut gpui::App) {
                self.0.borrow_mut().push(format!("begin {session_id}"));
            }
            fn finish(&self, session_id: &str, _: &mut gpui::App) {
                self.0.borrow_mut().push(format!("finish {session_id}"));
            }
            fn cancel(&self, session_id: &str, _: &mut gpui::App) {
                self.0.borrow_mut().push(format!("cancel {session_id}"));
            }
        }

        /// The session the fake was asked to begin.
        fn began(fake: &FakeVoice) -> String {
            let calls = fake.0.borrow();
            let session = calls.iter().find_map(|call| call.strip_prefix("begin "));
            session.expect("dictation began").to_owned()
        }

        /// M4 review P1: while dictating, a status line says what it's doing; its Cancel is the Tab stop after the
        /// field, ends the dictation and gives the keyboard back to the field (which reports its focus, review C4).
        #[gpui::test]
        fn dictation_says_what_it_is_doing_and_cancel_ends_it(cx: &mut TestAppContext) {
            let (cx, _) = app(cx, Route::Chat("b1".into()), chat(turns(2), false), sends());
            let fake = Rc::new(FakeVoice::default());
            let backend: Rc<dyn VoiceBackend> = fake.clone();
            cx.update(|_, cx| voice::install(backend, cx));
            click(cx, "composer.b1.dictate");
            let session = began(&fake);
            assert_eq!(text("composer.b1.dictation"), "Preparing voice…");
            for (phase, says) in [
                (SpeechPhase::Listening, "Listening…"),
                (SpeechPhase::Transcribing, "Transcribing…"),
            ] {
                let event = SpeechEvent::Session {
                    session_id: session.clone(),
                    phase,
                    problem: None,
                };
                cx.update(|_, cx| voice::emit(event, cx));
                draw(cx);
                assert_eq!(text("composer.b1.dictation"), says);
            }
            click(cx, "composer.b1.field");
            assert!(
                node("composer.b1.field").unwrap().focused,
                "the field reports its focus"
            );
            cx.simulate_keystrokes("tab");
            draw(cx);
            let cancel = node("composer.b1.dictation.cancel").unwrap();
            assert!(cancel.focused, "Cancel is the next Tab stop");
            assert_eq!(cancel.text, "Cancel dictation");
            cx.simulate_keystrokes("enter");
            draw(cx);
            assert!(fake.0.borrow().contains(&format!("cancel {session}")));
            assert!(node("composer.b1.dictation").is_none(), "it went");
            assert!(
                node("composer.b1.field").unwrap().focused,
                "the keyboard is back in the field"
            );
        }

        /// A microphone that wouldn't open: the runtime cancels the session first, then dictation says why.
        #[gpui::test]
        fn a_dictation_that_could_not_start_says_why(cx: &mut TestAppContext) {
            let (cx, _) = app(cx, Route::Chat("b1".into()), chat(turns(2), false), sends());
            let fake = Rc::new(FakeVoice::default());
            let backend: Rc<dyn VoiceBackend> = fake.clone();
            cx.update(|_, cx| voice::install(backend, cx));
            click(cx, "composer.b1.dictate");
            let session = began(&fake);
            let why = "No microphone could be opened.";
            for (phase, problem) in [
                (SpeechPhase::Listening, None),
                (SpeechPhase::Cancelled, None),
                (SpeechPhase::Error, Some(why)),
            ] {
                let event = SpeechEvent::Session {
                    session_id: session.clone(),
                    phase,
                    problem: problem.map(|message| SpeechProblem {
                        code: "start_failed".into(),
                        message: message.into(),
                    }),
                };
                cx.update(|_, cx| voice::emit(event, cx));
            }
            draw(cx);
            assert!(node("composer.b1.dictation").is_none(), "it ended");
            assert_eq!(text("composer.b1.voice"), why);
        }

        #[gpui::test]
        fn dictation_never_sends(cx: &mut TestAppContext) {
            let (cx, calls) = app(cx, Route::Chat("b1".into()), chat(turns(2), false), sends());
            let fake = Rc::new(FakeVoice::default());
            let backend: Rc<dyn VoiceBackend> = fake.clone();
            cx.update(|_, cx| voice::install(backend, cx));
            click(cx, "composer.b1.field");
            cx.simulate_input("Please ");
            click(cx, "composer.b1.dictate");
            let session = fake
                .0
                .borrow()
                .first()
                .and_then(|call| call.strip_prefix("begin ").map(str::to_owned));
            let session = session.expect("dictation began");
            cx.update(|_, cx| {
                voice::emit(
                    SpeechEvent::Final {
                        session_id: session.clone(),
                        text: "run cargo test again".into(),
                    },
                    cx,
                )
            });
            draw(cx);
            assert!(
                calls_to(&calls, "chat.send").is_empty(),
                "the words landed; nothing was sent"
            );
            assert_eq!(node("composer.b1.voice").unwrap().text, TRANSCRIPT_ADDED);
            // Sending is still yours: Enter sends the draft with the words in it.
            click(cx, "composer.b1.field");
            cx.simulate_keystrokes("enter");
            draw(cx);
            assert_eq!(
                calls_to(&calls, "chat.send")[0]["text"],
                "Please run cargo test again"
            );
        }

        #[gpui::test]
        fn placeholder_body_size(cx: &mut TestAppContext) {
            let (cx, _) = app(
                cx,
                Route::Chat("b1".into()),
                chat(turns(2), false),
                FakeRpc::new(),
            );
            // The empty field shows its placeholder in the field's own text style: one row of the Aegis body type
            // (14.5 on 22) between the field's 6 and 10 px of padding.
            let field = cx.debug_bounds("composer.b1.field").unwrap();
            let body = cx.update(|_, cx| super::super::field_type(cx).clone());
            assert_eq!((body.size, body.line_height), (14.5, 22.));
            assert!(
                (f32::from(field.size.height) - (6. + body.line_height + 10.)).abs() < 1.,
                "{field:?}"
            );
            assert_eq!(node("composer.b1.field").unwrap().text, "Message Forge");
        }

        #[gpui::test]
        fn typing_at_offers_the_other_agents_and_enter_picks_one(cx: &mut TestAppContext) {
            let mut team = chat(turns(2), false);
            team.bots.push(bot("b2", "Scout", BotState::Idle));
            let (cx, calls) = app(cx, Route::Chat("b1".into()), team, sends());
            click(cx, "composer.b1.field");
            cx.simulate_input("Ask @Sc");
            draw(cx);
            let scout = node("composer.b1.mention.b2").expect("Scout is offered");
            assert_eq!(scout.role, "ListBoxOption");
            assert!(scout.selected, "the highlighted one is selected");
            assert!(
                node("composer.b1.mention.b1").is_none(),
                "not the agent you're talking to"
            );
            cx.simulate_keystrokes("enter");
            draw(cx);
            assert!(
                node("composer.b1.mentions").is_none(),
                "picking closes the list"
            );
            assert!(
                calls_to(&calls, "chat.send").is_empty(),
                "Enter picked; it didn't send"
            );
            cx.simulate_keystrokes("enter");
            draw(cx);
            let sent = calls_to(&calls, "chat.send");
            assert!(
                sent[0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("Ask @Scout"),
                "{sent:?}"
            );
        }

        #[gpui::test]
        fn an_empty_field_stops_a_working_agent_and_stopping_clears_nothing(
            cx: &mut TestAppContext,
        ) {
            let mut working = state(vec![bot("b1", "Forge", BotState::Working)], vec![], vec![]);
            working.conversations = chat(turns(2), false).conversations;
            let rpc = sends().on::<method::ChatStop>(|_| Ok(protocol::wire::Empty {}));
            let (cx, calls) = app(cx, Route::Chat("b1".into()), working, rpc);
            assert_eq!(text("composer.b1.round"), "Stop Forge");
            assert_eq!(
                node("composer.b1.hint").unwrap().text,
                "Forge is working. Your message joins this turn, or queues if it can't."
            );
            click(cx, "composer.b1.round");
            assert_eq!(calls_to(&calls, "chat.stop").len(), 1);
            // Enter never stops an agent: with text it sends (steering), and an empty field does nothing on Enter.
            click(cx, "composer.b1.field");
            cx.simulate_keystrokes("enter");
            draw(cx);
            assert_eq!(calls_to(&calls, "chat.stop").len(), 1);
            cx.simulate_input("Check the main branch");
            draw(cx);
            assert_eq!(
                text("composer.b1.round"),
                "Send",
                "with text, the round button sends"
            );
        }

        /// M4 review A4 / C1: Stop and Send are one button, so a focused Stop that turns into Send keeps the focus.
        /// With nothing to send, Send is unavailable and says so (review A-N5).
        #[gpui::test]
        fn the_round_button_keeps_the_focus_when_stop_turns_to_send(cx: &mut TestAppContext) {
            let mut working = state(vec![bot("b1", "Forge", BotState::Working)], vec![], vec![]);
            working.conversations = chat(turns(2), false).conversations;
            let (cx, _) = app(cx, Route::Chat("b1".into()), working, FakeRpc::new());
            let found = cx.update(|window, cx| {
                crate::ui::primitives::focus_by_id("composer.b1.round", window, cx)
            });
            assert!(found, "the round button is drawn");
            draw(cx);
            assert_eq!(text("composer.b1.round"), "Stop Forge");
            assert!(node("composer.b1.round").unwrap().focused);
            // The turn ends.
            cx.update(|_, cx| {
                store::update(cx, |state| {
                    state.bots[0].state = BotState::Idle;
                    true
                })
            });
            draw(cx);
            let round = node("composer.b1.round").unwrap();
            assert_eq!(round.text, "Send, unavailable");
            assert!(!round.enabled, "nothing to send");
            assert!(round.focused, "the same button, still focused");
        }

        /// M4 review A2: a model list that didn't come is asked for again on the next connection. A list is for one
        /// PC: another PC's chat asks for its own and never shows this one's.
        #[gpui::test]
        fn the_model_menu_fills_after_a_failed_list_and_a_new_connection(cx: &mut TestAppContext) {
            let mut lists = 0;
            let rpc = FakeRpc::new().on::<method::ModelsList>(move |_| {
                lists += 1;
                let model = |id: &str, label: &str| ModelInfo {
                    id: id.into(),
                    label: label.into(),
                    is_default: false,
                };
                match lists {
                    1 => Err(client::RpcError::Daemon {
                        code: protocol::ErrorCode::Unavailable,
                        message: "The models couldn't be listed.".into(),
                    }),
                    2 => Ok(vec![model("opus", "Opus")]),
                    _ => Ok(vec![model("haiku", "Haiku")]),
                }
            });
            let (cx, calls) = app(cx, Route::Chat("b1".into()), chat(turns(2), false), rpc);
            draw(cx);
            assert_eq!(
                calls_to(&calls, "models.list").len(),
                1,
                "asked once; it failed"
            );
            // The connection drops and comes back.
            for connection in [Connection::Reconnecting, Connection::Ready] {
                cx.update(|_, cx| {
                    store::update(cx, |state| {
                        state.connection = connection;
                        true
                    })
                });
                draw(cx);
                draw(cx);
            }
            assert_eq!(
                calls_to(&calls, "models.list").len(),
                2,
                "asked again once Ready"
            );
            click(cx, "composer.b1.model.open");
            assert!(node("composer.b1.model.opus").is_some(), "the menu fills");
            // Another PC's chat, for an agent with the same id.
            cx.update(|_, cx| {
                store::update(cx, |state| {
                    state.host = Some("pc-2".into());
                    true
                })
            });
            draw(cx);
            draw(cx);
            assert_eq!(calls_to(&calls, "models.list").len(), 3);
            assert!(
                node("composer.b1.model.opus").is_none(),
                "not this PC's list"
            );
            assert!(
                node("composer.b1.model.haiku").is_some(),
                "but that PC's own"
            );
            // There, a file of this PC's can't be attached: Attach is unavailable and says where to attach (A-N5).
            assert_eq!(
                text("composer.b1.attach"),
                "Attach files in Tjommie on that PC, unavailable"
            );
        }
    }

    #[test]
    fn an_edit_is_the_span_the_two_drafts_dont_share() {
        let edit = draft_edit("Run the tests", "Run all the tests");
        assert_eq!((edit.start, edit.end), (4, 4), "inserted after 'Run '");
        let edit = draft_edit("abc", "abc");
        assert_eq!((edit.start, edit.end), (3, 3));
        let edit = draft_edit("héllo", "hello");
        assert_eq!((edit.start, edit.end), (1, 2), "UTF-16 units");
        assert_eq!(byte_of("héllo", 2), 3, "'é' is two bytes");
    }
}
