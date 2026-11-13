//! Port of `packages/daemon/src/team.snapshots.test.ts`: the shared-file snapshot lifecycle of `Team`. An `it.each` is a
//! loop inside one test, with the row named in the assertion messages. Each row gets a fresh fixture, as `beforeEach`
//! gives each TypeScript case one.
//!
//! Spies, replaced by what the Windows filesystem does for real:
//! - `renameSync` made to fail its second call (`vi.mock`): the second snapshot folder is opened as a directory without
//!   `FILE_SHARE_DELETE`, so renaming it fails. The folder order is fixed, so the first moves and the second fails.
//! - `utimesSync` ageing folders: `File::set_modified` on a directory opened with `FILE_FLAG_BACKUP_SEMANTICS`.
//! - `console.error` (`vi.spyOn`): the fixture's `logs` recorder.
mod team_support;
use team_support::*;

use std::fs;
use std::path::{Path, PathBuf};

use daemon::bots::{StoredBot, delete_bot};
use daemon::chat::{get_message, history};
use daemon::queue::{NewWork, WorkItem, WorkSource, enqueue, new_chain};
use daemon::runner::{TurnOutcome, TurnResult};
use daemon::team::get_exchange;
use protocol::model::{ExchangeStatus, Message, MessageKind};
use serde_json::{Value, json};

const HERE_IS_THE_FILE: &str = "Here is the file.";

/// `{ ...done, outcome, error }`: `done`'s text, with this outcome and error.
fn turn(outcome: TurnOutcome, error: Option<&str>) -> TurnResult {
    TurnResult {
        outcome,
        text: Some(HERE_IS_THE_FILE.to_owned()),
        text_message_id: None,
        error: error.map(str::to_owned),
    }
}

/// `done`.
fn done() -> TurnResult {
    turn(TurnOutcome::Done, None)
}

/// `{ ...done, text: "Follow-up" }`.
fn follow_up() -> TurnResult {
    TurnResult {
        text: Some("Follow-up".to_owned()),
        ..done()
    }
}

/// `writer` and `asker` in the TypeScript: Asker may message Writer.
struct Bots {
    writer: StoredBot,
    asker: StoredBot,
}

fn bots(fx: &Fixture) -> Bots {
    let writer = fx.bot("Writer", "blue", "circle", "", "codex", "gpt-5.6-sol");
    let asker = fx.bot("Asker", "blue", "circle", "", "claude", "sonnet");
    let asker = fx.set_can_message(&asker.id, &[writer.id.as_str()]);
    Bots { writer, asker }
}

/// `getMessage(db, id)`.
fn stored_message(fx: &Fixture, id: &str) -> Message {
    fx.db(|db| get_message(db, id).unwrap())
}

/// `history(db, conversationId, undefined, limit)`: the newest `limit` messages, oldest first.
fn history_of(fx: &Fixture, conversation_id: &str, limit: u32) -> Vec<Message> {
    fx.db(|db| history(db, conversation_id, None, limit).unwrap())
}

/// `history(db, conversationId, undefined, 1)[0]`.
fn newest(fx: &Fixture, conversation_id: &str) -> Message {
    history_of(fx, conversation_id, 1).remove(0)
}

/// `card.data.path`.
fn data_path(message: &Message) -> String {
    message
        .data
        .as_ref()
        .and_then(|data| data.get("path"))
        .and_then(Value::as_str)
        .expect("a file card has a path")
        .to_owned()
}

/// `card.data.name`.
fn name_of(message: &Message) -> String {
    message
        .data
        .as_ref()
        .and_then(|data| data.get("name"))
        .and_then(Value::as_str)
        .expect("a file card has a name")
        .to_owned()
}

/// `dirname(card.data.path)`: the snapshot folder a card points at.
fn staging_of(card: &Message) -> PathBuf {
    Path::new(&data_path(card)).parent().unwrap().to_path_buf()
}

fn as_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `readdirSync(dir)`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// `SELECT * FROM exchanges`: every column of every row, in order.
fn exchange_rows(fx: &Fixture) -> Vec<Vec<rusqlite::types::Value>> {
    fx.db(|db| {
        let mut statement = db.prepare("SELECT * FROM exchanges").unwrap();
        let columns = statement.column_count();
        statement
            .query_map([], |row| {
                (0..columns)
                    .map(|index| row.get::<_, rusqlite::types::Value>(index))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    })
}

/// Every bot reply the team delivered so far.
fn bot_replies(fx: &Fixture) -> Vec<WorkItem> {
    recorded(&fx.delivered)
        .iter()
        .filter(|work| work.source == WorkSource::BotReply)
        .cloned()
        .collect()
}

fn last_delivered(fx: &Fixture) -> WorkItem {
    recorded(&fx.delivered)
        .last()
        .cloned()
        .expect("something was delivered")
}

/// `console.error(context, Error)` was called.
fn logged(fx: &Fixture, context: &str) -> bool {
    recorded(&fx.logs)
        .iter()
        .any(|(logged_context, error)| logged_context == context && !error.is_empty())
}

/// `currentWork.set(botId, item)`.
fn set_current(fx: &Fixture, bot_id: &str, item: WorkItem) {
    fx.current.lock().unwrap().insert(bot_id.to_owned(), item);
}

/// `ask()`: Asker asks Writer for a file, and Writer's current work is that request.
fn ask(fx: &Fixture, b: &Bots) -> WorkItem {
    let asking = fx.db(|db| {
        let chain_id = new_chain(db).unwrap();
        enqueue(
            db,
            &NewWork {
                chain_id: Some(chain_id),
                ..NewWork::new(&b.asker.id, WorkSource::You, "Help")
            },
        )
        .unwrap()
    });
    set_current(fx, &b.asker.id, asking);
    fx.run_tool(
        "message_bot",
        &b.asker.id,
        json!({ "bot": "Writer", "message": "Share a file." }),
    )
    .unwrap();
    let request = last_delivered(fx);
    set_current(fx, &b.writer.id, request.clone());
    request
}

/// `share(conversationId, name)`: Writer writes `name` in its home and shares it. Returns the card.
fn share(fx: &Fixture, b: &Bots, conversation_id: &str, name: &str) -> Message {
    fs::write(Path::new(&b.writer.home_folder).join(name), "snapshot contents").unwrap();
    fx.run_tool("share_file", &b.writer.id, json!({ "path": name }))
        .unwrap();
    newest(fx, conversation_id)
}

// ---- Team snapshot lifecycle ----

#[test]
fn refuses_a_share_if_the_protected_staging_folder_was_removed() {
    let fx = fixture();
    let b = bots(&fx);
    let source = Path::new(&b.writer.home_folder).join("unshared.txt");
    fs::write(&source, "fixture").unwrap();
    let staging = fx.shared_files();
    fs::remove_dir_all(&staging).unwrap();
    let error = fx
        .run_tool("share_file", &b.writer.id, json!({ "path": as_text(&source) }))
        .unwrap_err();
    assert!(
        error.contains("The protected shared-files folder is unavailable."),
        "{error}"
    );
    assert!(!staging.exists());
}

#[test]
fn keeps_files_from_a_failed_or_interrupted_turn_in_the_target_bot_inbox_and_releases_staging() {
    for (outcome, label) in [
        (TurnOutcome::Failed, "failed"),
        (TurnOutcome::Interrupted, "interrupted"),
    ] {
        let fx = fixture();
        let b = bots(&fx);
        let request = ask(&fx, &b);
        let card = share(&fx, &b, &request.conversation_id, "answer.txt");
        let stage = staging_of(&card);
        fx.team_call(|team, host| {
            team.turn_ended(host, &request, &turn(outcome, Some("Could not finish")))
        })
        .unwrap();
        let expected = Path::new(&b.writer.home_folder)
            .join("inbox")
            .join("answer.txt");
        assert_eq!(
            data_path(&stored_message(&fx, &card.id)),
            as_text(&expected),
            "{label}"
        );
        assert_eq!(
            fs::read_to_string(&expected).unwrap(),
            "snapshot contents",
            "{label}"
        );
        assert!(!stage.exists(), "{label}");
    }
}

#[test]
fn still_tells_the_asker_about_a_failed_or_interrupted_turn_when_saving_shared_files_fails() {
    for (outcome, label, reason) in [
        (TurnOutcome::Failed, "failed", "The CLI exited"),
        (TurnOutcome::Interrupted, "interrupted", "it was stopped"),
    ] {
        let fx = fixture();
        let b = bots(&fx);
        let request = ask(&fx, &b);
        let card = share(&fx, &b, &request.conversation_id, "answer.txt");
        let snapshot = data_path(&card);
        // The inbox becomes a file, so copying into it fails.
        let writer_home = PathBuf::from(&b.writer.home_folder);
        fs::remove_dir_all(writer_home.join("inbox")).unwrap();
        fs::write(writer_home.join("inbox"), "blocks copying").unwrap();
        fx.team_call(|team, host| {
            team.turn_ended(host, &request, &turn(outcome, Some("The CLI exited")))
        })
        .unwrap();
        let exchange_id = request.exchange_id.clone().unwrap();
        let exchange = fx.db(|db| get_exchange(db, &exchange_id).unwrap());
        assert_eq!(exchange.status, ExchangeStatus::Failed, "{label}");
        assert_eq!(exchange.reply.as_deref(), Some(reason), "{label}");
        let replies = bot_replies(&fx);
        assert_eq!(replies.len(), 1, "{label}");
        assert_eq!(replies[0].bot_id, b.asker.id, "{label}");
        assert_eq!(
            replies[0].exchange_id.as_deref(),
            Some(exchange_id.as_str()),
            "{label}"
        );
        assert!(
            replies[0].text.contains(&format!(
                "Writer couldn't finish your request \"Share a file.\": {reason}"
            )),
            "{label}"
        );
        assert!(
            logged(
                &fx,
                "Tjommie: could not save files from a failed exchange turn"
            ),
            "{label}"
        );
        assert_eq!(data_path(&stored_message(&fx, &card.id)), snapshot, "{label}");
        assert!(Path::new(&snapshot).exists(), "{label}");
    }
}

#[test]
fn preserves_the_original_reply_when_a_follow_up_copy_save_or_delivery_fails() {
    for failure in ["copy", "save", "delivery"] {
        let fx = fixture();
        let b = bots(&fx);
        let request = ask(&fx, &b);
        fx.team_call(|team, host| team.turn_ended(host, &request, &done()))
            .unwrap();
        let exchange_id = request.exchange_id.clone().unwrap();
        let original = fx.db(|db| get_exchange(db, &exchange_id).unwrap());
        match failure {
            "copy" => {
                share(&fx, &b, &request.conversation_id, "answer.txt");
                let asker_home = PathBuf::from(&b.asker.home_folder);
                fs::remove_dir_all(asker_home.join("inbox")).unwrap();
                fs::write(asker_home.join("inbox"), "blocks copying").unwrap();
                fx.team_call(|team, host| team.turn_ended(host, &request, &follow_up()))
                    .unwrap();
            }
            "save" => {
                // A trigger makes saving the follow-up's room post fail.
                let trigger = format!(
                    "CREATE TRIGGER reject_follow_up BEFORE INSERT ON messages WHEN NEW.conversation_id = 'team' AND NEW.author = '{}' BEGIN SELECT RAISE(ABORT, 'cannot save follow-up'); END",
                    b.writer.id
                );
                fx.db(|db| db.execute_batch(&trigger).unwrap());
                assert!(
                    fx.team_call(|team, host| team.turn_ended(host, &request, &follow_up()))
                        .is_err(),
                    "{failure}"
                );
            }
            _ => {
                *fx.deliver_fails.lock().unwrap() = Some("cannot deliver follow-up".to_owned());
                assert!(
                    fx.team_call(|team, host| team.turn_ended(host, &request, &follow_up()))
                        .is_err(),
                    "{failure}"
                );
            }
        }
        assert_eq!(
            fx.db(|db| get_exchange(db, &exchange_id).unwrap()),
            original,
            "{failure}"
        );
        let handoff = history_of(&fx, &b.asker.id, 20)
            .into_iter()
            .find(|message| message.kind == MessageKind::Handoff)
            .unwrap();
        assert_eq!(
            handoff.data.as_ref().and_then(|data| data.get("status")),
            Some(&json!("replied")),
            "{failure}"
        );
    }
}

#[test]
fn delivers_and_releases_a_snapshot_shared_while_retrying_a_failed_exchange() {
    let fx = fixture();
    let b = bots(&fx);
    let request = ask(&fx, &b);
    fx.team_call(|team, host| {
        team.turn_ended(host, &request, &turn(TurnOutcome::Failed, Some("Try again")))
    })
    .unwrap();
    let retry = fx.db(|db| {
        let chain_id = new_chain(db).unwrap();
        enqueue(
            db,
            &NewWork {
                conversation_id: Some(request.conversation_id.clone()),
                exchange_id: request.exchange_id.clone(),
                chain_id: Some(chain_id),
                ..NewWork::new(&b.writer.id, WorkSource::BotRequest, &request.text)
            },
        )
        .unwrap()
    });
    set_current(&fx, &b.writer.id, retry.clone());
    let card = share(&fx, &b, &retry.conversation_id, "answer.txt");
    let stage = staging_of(&card);
    fx.team_call(|team, host| team.turn_ended(host, &retry, &done()))
        .unwrap();
    let expected = Path::new(&b.asker.home_folder)
        .join("inbox")
        .join("answer.txt");
    assert_eq!(
        data_path(&stored_message(&fx, &card.id)),
        as_text(&expected)
    );
    assert_eq!(
        fs::read_to_string(&expected).unwrap(),
        "snapshot contents"
    );
    assert!(!stage.exists());
}

#[test]
fn removes_staging_after_all_copies_succeed_and_points_cards_and_room_files_at_the_recipient_inbox() {
    let fx = fixture();
    let b = bots(&fx);
    let request = ask(&fx, &b);
    let cards = [
        share(&fx, &b, &request.conversation_id, "one.txt"),
        share(&fx, &b, &request.conversation_id, "two.txt"),
    ];
    let stages: Vec<PathBuf> = cards.iter().map(staging_of).collect();
    assert!(stages.iter().all(|stage| stage.exists()));
    fx.team_call(|team, host| team.turn_ended(host, &request, &done()))
        .unwrap();
    let asker_inbox = Path::new(&b.asker.home_folder).join("inbox");
    for (index, card) in cards.iter().enumerate() {
        let name = if index == 0 { "one.txt" } else { "two.txt" };
        let expected = asker_inbox.join(name);
        let updated = stored_message(&fx, &card.id);
        assert_eq!(data_path(&updated), as_text(&expected), "card {index}");
        assert_eq!(
            fs::read_to_string(&expected).unwrap(),
            "snapshot contents",
            "card {index}"
        );
        assert!(!stages[index].exists(), "card {index}");
    }
    let room_files = newest(&fx, "team")
        .data
        .as_ref()
        .and_then(|data| data.get("files"))
        .cloned();
    let expected_files: Vec<Value> = cards
        .iter()
        .map(|card| {
            json!({
                "name": name_of(card),
                "path": as_text(&asker_inbox.join(name_of(card))),
            })
        })
        .collect();
    assert_eq!(room_files, Some(Value::Array(expected_files)));
    assert!(names(&fx.shared_files()).is_empty());
}

#[test]
fn uses_a_durable_inbox_copy_and_immediately_removes_staging_for_a_share_in_the_bot_chat() {
    let fx = fixture();
    let b = bots(&fx);
    let card = share(&fx, &b, &b.writer.id, "answer.txt");
    let writer_home = PathBuf::from(&b.writer.home_folder);
    let expected = writer_home.join("inbox").join("answer.txt");
    assert_eq!(data_path(&card), as_text(&expected));
    fs::remove_file(writer_home.join("answer.txt")).unwrap();
    assert_eq!(
        fs::read_to_string(&expected).unwrap(),
        "snapshot contents"
    );
    assert!(names(&fx.shared_files()).is_empty());
}

#[test]
fn keeps_failed_delivery_staging_pending_until_successful_bot_deletion_removes_it() {
    let fx = fixture();
    let b = bots(&fx);
    let request = ask(&fx, &b);
    let card = share(&fx, &b, &request.conversation_id, "answer.txt");
    let staging = staging_of(&card);
    let asker_home = PathBuf::from(&b.asker.home_folder);
    fs::remove_dir_all(asker_home.join("inbox")).unwrap();
    fs::write(asker_home.join("inbox"), "blocks copying").unwrap();
    fx.team_call(|team, host| team.turn_ended(host, &request, &done()))
        .unwrap();
    assert!(staging.exists());
    let handle = fx.team_call(|team, host| team.bot_deleting(host, &b.writer.id).unwrap());
    assert!(staging.exists());
    fx.db(|db| delete_bot(db, &b.writer.id).unwrap());
    fx.team_call(|team, host| team.bot_deleted(host, &handle).unwrap());
    assert!(!staging.exists());
}

#[test]
fn retains_pending_snapshots_when_deletion_finalization_rolls_back() {
    let fx = fixture();
    let b = bots(&fx);
    let request = ask(&fx, &b);
    let card = share(&fx, &b, &request.conversation_id, "answer.txt");
    let staging = staging_of(&card);
    let handle = fx.team_call(|team, host| team.bot_deleting(host, &b.writer.id).unwrap());
    fx.db(|db| delete_bot(db, &b.writer.id).unwrap());
    fx.db(|db| {
        db.execute_batch(
            "CREATE TRIGGER reject_deletion BEFORE INSERT ON work_items WHEN NEW.source = 'bot_reply' BEGIN SELECT RAISE(ABORT, 'cannot notify'); END",
        )
        .unwrap()
    });
    let error = fx
        .team_call(|team, host| team.bot_deleted(host, &handle))
        .unwrap_err();
    assert!(error.message.contains("cannot notify"), "{}", error.message);
    assert!(staging.exists());
    fx.db(|db| db.execute_batch("DROP TRIGGER reject_deletion").unwrap());
    fx.team_call(|team, host| team.bot_deleted(host, &handle).unwrap());
    assert!(!staging.exists());
}

/// `FILE_SHARE_READ` alone. Nothing may open the folder with `FILE_SHARE_DELETE`, so it cannot be renamed.
#[cfg(windows)]
const FILE_SHARE_READ: u32 = 0x0000_0001;
/// `FILE_FLAG_BACKUP_SEMANTICS`: lets `CreateFile` open a directory.
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

#[cfg(windows)]
#[test]
fn restores_moved_snapshots_and_database_state_when_a_later_cleanup_rename_fails() {
    use std::os::windows::fs::OpenOptionsExt;

    let fx = fixture();
    let b = bots(&fx);
    let request = ask(&fx, &b);
    let cards = [
        share(&fx, &b, &request.conversation_id, "one.txt"),
        share(&fx, &b, &request.conversation_id, "two.txt"),
    ];
    let folders: Vec<PathBuf> = cards.iter().map(staging_of).collect();
    let mut handle = fx.team_call(|team, host| team.bot_deleting(host, &b.writer.id).unwrap());
    fx.db(|db| delete_bot(db, &b.writer.id).unwrap());
    // The renames run in the order the handle lists the folders. Fix that order: the first folder moves, then the
    // second, which is locked, fails.
    handle.snapshot_folders = folders.clone();
    let before = exchange_rows(&fx);
    let sent_before = recorded(&fx.delivered).len();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(&folders[1])
        .unwrap();
    assert!(fx
        .team_call(|team, host| team.bot_deleted(host, &handle))
        .is_err());
    assert_eq!(exchange_rows(&fx), before);
    assert_eq!(recorded(&fx.delivered).len(), sent_before);
    assert!(folders.iter().all(|folder| folder.exists()));
    assert!(names(&fx.shared_files())
        .iter()
        .all(|name| name.starts_with("share-")));
    drop(lock);
    fx.team_call(|team, host| team.bot_deleted(host, &handle).unwrap());
    assert!(!folders.iter().any(|folder| folder.exists()));
}

/// Sets a folder's modified time, which `Team::start` reads. A directory opens for its times only as a backup handle.
#[cfg(windows)]
fn set_folder_time(folder: &Path, when: std::time::SystemTime) {
    use std::os::windows::fs::OpenOptionsExt;

    let handle = fs::OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(folder)
        .unwrap();
    handle.set_modified(when).unwrap();
}

#[cfg(windows)]
#[test]
fn sweeps_only_orphan_snapshot_folders_older_than_24_hours_at_startup() {
    use std::time::{Duration, SystemTime};

    let fx = fixture();
    let staging = fx.shared_files();
    let old = staging.join("share-orphan");
    let cleanup = staging.join("cleanup-orphan");
    let recent = staging.join("share-recent");
    let unrelated = staging.join("other-folder");
    for folder in [&old, &cleanup, &recent, &unrelated] {
        fs::create_dir_all(folder).unwrap();
        fs::write(folder.join("file.txt"), "keep or sweep").unwrap();
    }
    let yesterday = SystemTime::now() - Duration::from_secs(25 * 60 * 60);
    for folder in [&old, &cleanup, &unrelated] {
        set_folder_time(folder, yesterday);
    }
    fx.team.start();
    assert!(!old.exists());
    assert!(!cleanup.exists());
    assert!(recent.exists());
    assert!(unrelated.exists());
    fx.team.start();
}
