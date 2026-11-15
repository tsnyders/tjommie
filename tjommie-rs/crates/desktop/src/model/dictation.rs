//! Port of `packages/ui/lib/dictation.ts` (T1.4), and the logic of `use-dictation.ts` that isn't React or the speech bridge:
//! how a draft edit moves the range a transcript goes to, where a transcript goes, which language a start uses, what a
//! speech event does to the draft, and what a model action leaves behind. Positions are UTF-16 code units, as the DOM's
//! selection and input events give them. Not ported: the hook's effects (the bridge, PCM capture, the 59.75 s listen
//! timer, `localStorage`, the device list, React state); the desktop drives these decisions from its own speech bridge.

use protocol::speech::{
    SpeechCapture, SpeechEngine, SpeechEvent, SpeechModelState, SpeechPermission, SpeechPhase,
    SpeechStatus,
};

/// Where a transcript goes in the draft, in UTF-16 units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsertionRange {
    pub start: usize,
    pub end: usize,
}

/// One browser edit of the draft: the text it began from, and the span it replaced (UTF-16 units).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftEdit {
    pub before: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insertion {
    pub text: String,
    pub caret: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictationError {
    InsertionMoved,
}

impl std::fmt::Display for DictationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("The dictation insertion point changed.")
    }
}

impl std::error::Error for DictationError {}

/// `text.length`: UTF-16 code units.
fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The byte offset of UTF-16 unit `unit`. `None` when it falls inside a surrogate pair, or past the end.
fn byte_at(text: &str, unit: usize) -> Option<usize> {
    let mut seen = 0;
    for (index, character) in text.char_indices() {
        if seen == unit {
            return Some(index);
        }
        seen += character.len_utf16();
        if seen > unit {
            return None;
        }
    }
    (seen == unit).then_some(text.len())
}

/// `text.slice(start, end)` in UTF-16 units. `None` where a unit falls inside a surrogate pair.
fn slice16(text: &str, start: usize, end: usize) -> Option<&str> {
    let from = byte_at(text, start)?;
    let to = byte_at(text, end)?;
    text.get(from..to)
}

/// Two slices that are both there and equal. A slice that can't be taken never matches.
fn same(left: Option<&str>, right: Option<&str>) -> bool {
    matches!((left, right), (Some(a), Some(b)) if a == b)
}

const EDIT_TYPES: [&str; 9] = [
    "insertText",
    "insertFromPaste",
    "insertFromDrop",
    "insertReplacementText",
    "insertLineBreak",
    "insertParagraph",
    "deleteContentBackward",
    "deleteContentForward",
    "deleteByCut",
];

/// `inputEdit`: what a `beforeinput` changes. `selection` is the field's (`None` puts the caret at the end), `input_type` is
/// the event's `inputType` (`None` when it isn't a string). Composition and history edits give `None`.
pub fn input_edit(
    value: &str,
    selection: Option<(usize, usize)>,
    input_type: Option<&str>,
    is_composing: bool,
) -> Option<DraftEdit> {
    let length = utf16_len(value);
    let (mut start, mut end) = selection.unwrap_or((length, length));
    let input_type = input_type?;
    if is_composing || input_type.contains("Composition") {
        return None;
    }
    if start == end && input_type == "deleteContentBackward" {
        start = start.saturating_sub(1);
    } else if start == end && input_type == "deleteContentForward" {
        end = (end + 1).min(length);
    } else if !EDIT_TYPES.contains(&input_type) {
        return None;
    }
    Some(DraftEdit {
        before: value.to_owned(),
        start,
        end,
    })
}

/// `moveRange`: where the range is once `edit` has turned `before` into `after`. An edit that overlaps the range, or doesn't
/// match the text it claims to change, leaves no range.
pub fn move_range(
    range: Option<InsertionRange>,
    edit: Option<&DraftEdit>,
    before: &str,
    after: &str,
) -> Option<InsertionRange> {
    let (range, edit) = (range?, edit?);
    if edit.before != before {
        return None;
    }
    let removed = edit.end.checked_sub(edit.start)?;
    // after.length - (before.length - removed), which must not be negative.
    let inserted = utf16_len(after)
        .checked_add(removed)?
        .checked_sub(utf16_len(before))?;
    if !same(
        slice16(after, 0, edit.start),
        slice16(before, 0, edit.start),
    ) {
        return None;
    }
    let tail_from = edit.start.checked_add(inserted)?;
    if !same(
        slice16(after, tail_from, utf16_len(after)),
        slice16(before, edit.end, utf16_len(before)),
    ) {
        return None;
    }
    if edit.end <= range.start {
        let shift = |position: usize| -> Option<usize> {
            position.checked_add(inserted)?.checked_sub(removed)
        };
        return Some(InsertionRange {
            start: shift(range.start)?,
            end: shift(range.end)?,
        });
    }
    (edit.start >= range.end).then_some(range)
}

/// `insertTranscript`: the draft with `transcript` in place of `range`, and the caret after it. A range the draft no longer
/// has is an error (the TypeScript throws).
pub fn insert_transcript(
    text: &str,
    range: InsertionRange,
    transcript: &str,
) -> Result<Insertion, DictationError> {
    if range.end < range.start || range.end > utf16_len(text) {
        return Err(DictationError::InsertionMoved);
    }
    let from = byte_at(text, range.start).ok_or(DictationError::InsertionMoved)?;
    let to = byte_at(text, range.end).ok_or(DictationError::InsertionMoved)?;
    let head = text.get(..from).ok_or(DictationError::InsertionMoved)?;
    let tail = text.get(to..).ok_or(DictationError::InsertionMoved)?;
    Ok(Insertion {
        text: format!("{head}{transcript}{tail}"),
        caret: range.start + utf16_len(transcript),
    })
}

/// The beforeinput check: an edit that touches the session's range, or an edit that isn't known, drops the range.
pub fn edit_clips_range(edit: Option<&DraftEdit>, range: InsertionRange) -> bool {
    edit.is_none_or(|edit| edit.end > range.start && edit.start < range.end)
}

pub const DEFAULT_LANGUAGE: &str = "en-US";
const LANGUAGE_MAX: usize = 40;
const NO_LANGUAGE: &str = "No installed local language is available. Check system speech settings.";
const DEFAULT_FAILURE: &str = "Dictation could not finish. Try again.";
pub const TRANSCRIPT_ADDED: &str = "Transcript added to your draft. Review it before sending.";
pub const TRANSCRIPT_MOVED: &str = "Your draft changed at the insertion point. Choose where to insert this transcript, or discard it.";
pub const PREPARED_MESSAGE: &str = "Android prepared English (United States) for this foreground session. Start dictation checks it again before recording.";

/// A saved language is kept only if it is `en` followed by up to 40 characters of `-` groups of 2 to 8 letters or digits.
pub fn preferred_language(saved: &str) -> String {
    let mut parts = saved.split('-');
    let valid = saved.len() <= LANGUAGE_MAX
        && parts.next() == Some("en")
        && parts.all(|part| {
            (2..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        });
    if valid {
        saved.to_owned()
    } else {
        DEFAULT_LANGUAGE.to_owned()
    }
}

/// Where a start goes: refused, with the reason, or started in `language`. `native` means Android's own capture, whose
/// status is read again afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartPlan {
    Refuse { message: String },
    Start { language: String, native: bool },
}

/// The start decision: the chosen language if the status lists it, else the first listed one; without a list, the English
/// the prepared or permission-asking Android state implies; refused unless the speech is available or only needs permission.
pub fn start_plan(status: &SpeechStatus, chosen: &str, saved: &str) -> StartPlan {
    let native_capture =
        status.engine == SpeechEngine::AndroidOnDevice && status.capture == SpeechCapture::Native;
    let permission_required = native_capture
        && status.model.state == SpeechModelState::SystemManaged
        && !status.available
        && status.languages.is_empty()
        && status
            .problem
            .as_ref()
            .is_some_and(|problem| problem.code == "permission_required")
        && matches!(
            status.permission,
            SpeechPermission::Prompt | SpeechPermission::Unknown
        );
    let prepared = status.available
        && native_capture
        && status.permission == SpeechPermission::Granted
        && status.model.state == SpeechModelState::SystemManaged
        && status
            .model
            .confirmation
            .as_ref()
            .is_some_and(|confirmation| {
                confirmation.source == "android-download-success"
                    && confirmation.language == DEFAULT_LANGUAGE
            });
    let listed = status
        .languages
        .iter()
        .find(|language| language.tag == chosen)
        .or_else(|| status.languages.first())
        .map(|language| language.tag.clone());
    let selected = listed.or_else(|| {
        if prepared {
            Some(DEFAULT_LANGUAGE.to_owned())
        } else if permission_required {
            Some(preferred_language(saved))
        } else {
            None
        }
    });
    match selected {
        Some(language) if status.available || permission_required => StartPlan::Start {
            language,
            native: native_capture,
        },
        _ => StartPlan::Refuse {
            message: status
                .problem
                .as_ref()
                .map_or(NO_LANGUAGE.to_owned(), |problem| problem.message.clone()),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    Idle,
    Permission,
    Listening,
    Transcribing,
}

/// What a speech event does to the draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechOutcome {
    /// The transcript goes into the draft at the session's range.
    Commit {
        text: String,
        message: String,
    },
    /// The draft moved away from the range, so the transcript waits for the user to place it.
    Hold {
        text: String,
        message: String,
    },
    Transcribing,
    Failed {
        message: String,
    },
    Cancelled,
}

/// The effect of a speech event on a session in `phase`, whose range survived if `has_range`. `None` when the event changes
/// nothing for this session (a status update is the caller's; a final text after the session left transcribing is stale).
pub fn speech_outcome(
    phase: SessionPhase,
    has_range: bool,
    event: &SpeechEvent,
) -> Option<SpeechOutcome> {
    match event {
        SpeechEvent::Status { .. } => None,
        SpeechEvent::Final { text, .. } => {
            if phase != SessionPhase::Transcribing {
                return None;
            }
            Some(if has_range {
                SpeechOutcome::Commit {
                    text: text.clone(),
                    message: TRANSCRIPT_ADDED.to_owned(),
                }
            } else {
                SpeechOutcome::Hold {
                    text: text.clone(),
                    message: TRANSCRIPT_MOVED.to_owned(),
                }
            })
        }
        SpeechEvent::Session {
            phase: speech,
            problem,
            ..
        } => match speech {
            SpeechPhase::Transcribing => Some(SpeechOutcome::Transcribing),
            SpeechPhase::Error => Some(SpeechOutcome::Failed {
                message: problem
                    .as_ref()
                    .map_or(DEFAULT_FAILURE.to_owned(), |problem| {
                        problem.message.clone()
                    }),
            }),
            SpeechPhase::Cancelled => Some(SpeechOutcome::Cancelled),
            SpeechPhase::Listening => None,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelAction {
    Download,
    CancelDownload,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOutcome {
    pub message: String,
    /// The problem to show on the draft, when the model still isn't usable.
    pub issue: Option<String>,
}

/// Whether a model action may start: not while a session runs, and one at a time, except that a download can be cancelled.
pub fn model_action_allowed(
    session_active: bool,
    model_pending: bool,
    action: ModelAction,
) -> bool {
    !session_active && !(model_pending && action != ModelAction::CancelDownload)
}

/// What a finished model action leaves on screen, and the issue it leaves on the draft.
pub fn model_outcome(action: ModelAction, status: &SpeechStatus) -> ModelOutcome {
    let problem = status
        .problem
        .as_ref()
        .map(|problem| problem.message.clone());
    let fallback = match action {
        ModelAction::CancelDownload => "Model download cancelled.",
        ModelAction::Download | ModelAction::Remove => "Speech model status refreshed.",
    };
    let message = if status.model.confirmation.is_some() {
        PREPARED_MESSAGE.to_owned()
    } else {
        problem.clone().unwrap_or_else(|| fallback.to_owned())
    };
    let issue = if status.available { None } else { problem };
    ModelOutcome { message, issue }
}

#[cfg(test)]
mod tests {
    use protocol::speech::{SpeechEvent, SpeechStatus};
    use serde_json::json;

    use super::*;

    fn range(start: usize, end: usize) -> InsertionRange {
        InsertionRange { start, end }
    }

    fn status(value: serde_json::Value) -> SpeechStatus {
        serde_json::from_value(value).unwrap()
    }

    fn pcm_ready() -> SpeechStatus {
        status(json!({
            "engine": "whisper.cpp", "capture": "pcm", "available": true,
            "languages": [{ "tag": "en-US", "label": "English (United States)" }],
            "permission": "granted", "model": { "state": "ready", "bytes": 0, "received": 0 }
        }))
    }

    fn android(
        available: bool,
        permission: &str,
        languages: serde_json::Value,
        problem: serde_json::Value,
    ) -> SpeechStatus {
        status(json!({
            "engine": "android-on-device", "capture": "native", "available": available,
            "languages": languages, "permission": permission,
            "model": { "state": "system-managed", "bytes": 0, "received": 0 },
            "problem": problem
        }))
    }

    #[test]
    fn moves_the_original_range_across_typed_edits_before_and_after_it() {
        let before = "a OLD z";
        let edit = DraftEdit {
            before: before.to_owned(),
            start: 0,
            end: 0,
        };
        let moved = move_range(Some(range(2, 5)), Some(&edit), before, "typed a OLD z");
        assert_eq!(moved, Some(range(8, 11)));
        let later = DraftEdit {
            before: "typed a OLD z".to_owned(),
            start: 12,
            end: 13,
        };
        assert_eq!(
            move_range(moved, Some(&later), "typed a OLD z", "typed a OLD end"),
            moved
        );
        assert_eq!(
            insert_transcript("typed a OLD end", moved.unwrap_or(range(0, 0)), "words"),
            Ok(Insertion {
                text: "typed a words end".to_owned(),
                caret: 13
            })
        );
    }

    #[test]
    fn refuses_overlapping_ambiguous_ime_and_same_text_replacement_edits() {
        let overlapping = DraftEdit {
            before: "OLD".to_owned(),
            start: 0,
            end: 3,
        };
        let different = DraftEdit {
            before: "different".to_owned(),
            start: 0,
            end: 0,
        };
        for edit in [None, Some(&overlapping), Some(&different)] {
            assert_eq!(move_range(Some(range(0, 3)), edit, "OLD", "OLD"), None);
        }
        let typed = DraftEdit {
            before: "aaaa".to_owned(),
            start: 0,
            end: 0,
        };
        assert_eq!(
            move_range(Some(range(1, 2)), Some(&typed), "aaaa", "aaaaa"),
            Some(range(2, 3))
        );
    }

    #[test]
    fn keeps_typing_at_a_collapsed_anchor_before_the_final_transcript() {
        let edit = DraftEdit {
            before: "ab".to_owned(),
            start: 1,
            end: 1,
        };
        assert_eq!(
            move_range(Some(range(1, 1)), Some(&edit), "ab", "a typed b"),
            Some(range(8, 8))
        );
        assert_eq!(
            insert_transcript("a", range(1, 2), "words"),
            Err(DictationError::InsertionMoved)
        );
    }

    #[test]
    fn maps_ordinary_browser_edits_and_refuses_composition_and_history_edits() {
        assert_eq!(
            input_edit("hello", Some((3, 3)), Some("deleteContentBackward"), false),
            Some(DraftEdit {
                before: "hello".to_owned(),
                start: 2,
                end: 3
            })
        );
        assert_eq!(
            input_edit("hello", Some((3, 3)), Some("historyUndo"), false),
            None
        );
        assert_eq!(
            input_edit("hello", Some((3, 3)), Some("insertCompositionText"), true),
            None
        );
    }

    #[test]
    fn counts_positions_in_utf_16_units_as_the_dom_does() {
        // "😀 café" is seven UTF-16 units: the emoji takes two.
        let text = "\u{1F600} caf\u{e9}";
        assert_eq!(
            insert_transcript(text, range(3, 3), "x").map(|done| done.text),
            Ok("\u{1F600} xcaf\u{e9}".to_owned())
        );
        assert_eq!(
            insert_transcript(text, range(3, 3), "x").map(|done| done.caret),
            Ok(4)
        );
        assert_eq!(
            insert_transcript(text, range(1, 1), "x"),
            Err(DictationError::InsertionMoved)
        );
        assert_eq!(
            insert_transcript(text, range(0, 8), "x"),
            Err(DictationError::InsertionMoved)
        );
        let deleted = input_edit(text, Some((7, 7)), Some("deleteContentBackward"), false);
        assert_eq!(deleted.map(|edit| (edit.start, edit.end)), Some((6, 7)));
    }

    #[test]
    fn keeps_only_the_english_language_a_saved_preference_may_name() {
        assert_eq!(preferred_language("en-GB"), "en-GB");
        assert_eq!(preferred_language("en-GB-oxendict"), "en-GB-oxendict");
        assert_eq!(preferred_language("en"), "en");
        assert_eq!(preferred_language("fr-FR"), "en-US");
        assert_eq!(preferred_language("en-"), "en-US");
        assert_eq!(preferred_language("en-x"), "en-US");
        assert_eq!(
            preferred_language(&format!("en-{}", "a".repeat(40))),
            "en-US"
        );
        assert_eq!(preferred_language(""), "en-US");
    }

    #[test]
    fn starts_in_the_language_the_user_chose_or_the_first_installed_one() {
        let two = android(
            true,
            "granted",
            json!([{ "tag": "en-GB", "label": "UK" }, { "tag": "en-US", "label": "US" }]),
            json!(null),
        );
        assert_eq!(
            start_plan(&two, "en-US", "en-GB"),
            StartPlan::Start {
                language: "en-US".to_owned(),
                native: true
            }
        );
        assert_eq!(
            start_plan(&two, "", "en-GB"),
            StartPlan::Start {
                language: "en-GB".to_owned(),
                native: true
            }
        );
        assert_eq!(
            start_plan(&pcm_ready(), "", "en-GB"),
            StartPlan::Start {
                language: "en-US".to_owned(),
                native: false
            }
        );
    }

    #[test]
    fn refuses_a_start_with_nothing_installed_and_says_why() {
        let refused = android(
            false,
            "granted",
            json!([]),
            json!({ "code": "no_language", "message": "Install a voice pack." }),
        );
        assert_eq!(
            start_plan(&refused, "", "en-US"),
            StartPlan::Refuse {
                message: "Install a voice pack.".to_owned()
            }
        );
        let silent = android(false, "denied", json!([]), json!(null));
        assert_eq!(
            start_plan(&silent, "", "en-US"),
            StartPlan::Refuse {
                message: "No installed local language is available. Check system speech settings."
                    .to_owned()
            }
        );
    }

    #[test]
    fn asks_for_permission_through_the_native_path_when_android_needs_it() {
        let asking = android(
            false,
            "prompt",
            json!([]),
            json!({ "code": "permission_required", "message": "Allow the microphone." }),
        );
        assert_eq!(
            start_plan(&asking, "", "en-GB"),
            StartPlan::Start {
                language: "en-GB".to_owned(),
                native: true
            }
        );
    }

    #[test]
    fn reports_a_speech_event_for_the_draft_only_while_it_is_transcribing() {
        let final_text = |text: &str| SpeechEvent::Final {
            session_id: "s".to_owned(),
            text: text.to_owned(),
        };
        let phase = |phase: &str, problem: serde_json::Value| -> SpeechEvent {
            serde_json::from_value(
                json!({ "kind": "session", "sessionId": "s", "phase": phase, "problem": problem }),
            )
            .unwrap()
        };
        assert_eq!(
            speech_outcome(SessionPhase::Transcribing, true, &final_text("hi")),
            Some(SpeechOutcome::Commit {
                text: "hi".to_owned(),
                message: TRANSCRIPT_ADDED.to_owned()
            })
        );
        assert_eq!(
            speech_outcome(SessionPhase::Transcribing, false, &final_text("hi")),
            Some(SpeechOutcome::Hold {
                text: "hi".to_owned(),
                message: TRANSCRIPT_MOVED.to_owned()
            })
        );
        assert_eq!(
            speech_outcome(SessionPhase::Listening, true, &final_text("hi")),
            None
        );
        assert_eq!(
            speech_outcome(
                SessionPhase::Listening,
                true,
                &phase("transcribing", json!(null))
            ),
            Some(SpeechOutcome::Transcribing)
        );
        assert_eq!(
            speech_outcome(
                SessionPhase::Listening,
                true,
                &phase("error", json!({ "code": "x", "message": "Mic is busy." }))
            ),
            Some(SpeechOutcome::Failed {
                message: "Mic is busy.".to_owned()
            })
        );
        assert_eq!(
            speech_outcome(SessionPhase::Listening, true, &phase("error", json!(null))),
            Some(SpeechOutcome::Failed {
                message: "Dictation could not finish. Try again.".to_owned()
            })
        );
        assert_eq!(
            speech_outcome(
                SessionPhase::Listening,
                true,
                &phase("cancelled", json!(null))
            ),
            Some(SpeechOutcome::Cancelled)
        );
    }

    #[test]
    fn drops_the_session_range_an_edit_touches_and_keeps_the_rest() {
        let typing_after = DraftEdit {
            before: "abcdef".to_owned(),
            start: 4,
            end: 4,
        };
        let inside = DraftEdit {
            before: "abcdef".to_owned(),
            start: 2,
            end: 3,
        };
        assert!(!edit_clips_range(Some(&typing_after), range(1, 3)));
        assert!(edit_clips_range(Some(&inside), range(1, 3)));
        assert!(edit_clips_range(None, range(1, 3)));
    }

    #[test]
    fn says_what_a_model_action_left_behind() {
        let prepared = status(json!({
            "engine": "android-on-device", "capture": "native", "available": true, "languages": [],
            "permission": "granted",
            "model": { "state": "system-managed", "bytes": 0, "received": 0,
                "confirmation": { "language": "en-US", "source": "android-download-success" } }
        }));
        assert_eq!(
            model_outcome(ModelAction::Download, &prepared).message,
            PREPARED_MESSAGE
        );
        let missing = status(json!({
            "engine": "whisper.cpp", "capture": "pcm", "available": false, "languages": [], "permission": "unknown",
            "model": { "state": "missing", "bytes": 0, "received": 0 },
            "problem": { "code": "model_missing", "message": "The speech model is not installed." }
        }));
        let outcome = model_outcome(ModelAction::Remove, &missing);
        assert_eq!(outcome.message, "The speech model is not installed.");
        assert_eq!(
            outcome.issue,
            Some("The speech model is not installed.".to_owned())
        );
        assert_eq!(
            model_outcome(ModelAction::CancelDownload, &pcm_ready()).message,
            "Model download cancelled."
        );
        assert_eq!(
            model_outcome(ModelAction::Remove, &pcm_ready()).message,
            "Speech model status refreshed."
        );
        assert!(!model_action_allowed(true, false, ModelAction::Download));
        assert!(!model_action_allowed(false, true, ModelAction::Remove));
        assert!(model_action_allowed(
            false,
            true,
            ModelAction::CancelDownload
        ));
    }
}
