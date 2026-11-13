//! Port of `packages/daemon/src/team.attachments.test.ts`: `Team` linking file cards to attachments. The real `Attachments`
//! locks its own database handle, which the host already holds, so these cases link through the fixture's
//! [`DbAttachments`] over the real `attachments` and `message_attachments` tables.
//!
//! Where the TypeScript reads through the service: `service.metadata(ref)` becomes a `message_attachments` lookup
//! ([`link_of`]); `service.open`/`read` become the stored file in `shared-files\attachments`, checked against the row's
//! sha256 ([`stored_bytes`]). What is Attachments' own (transfers, `close`, `backfillLegacy`, and companion validation
//! beyond what the team checks) is lane D's and is not ported here.
//!
//! Not ported literally:
//! - The broadcast's metadata lookup at the moment of each file announcement reads the links `DbAttachments` has recorded,
//!   because a broadcast must not take the host's database lock.
//! - `closeTransfer`, `close` and `backfillLegacy`: `DbAttachments` has no transfers or legacy rows. The restart is a fresh
//!   `DbAttachments` over the same folder, whose `cleanup` runs as the service's does at start.
mod team_support;
use team_support::*;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use daemon::bots::StoredBot;
use daemon::chat::history;
use daemon::queue::{NewWork, WorkItem, WorkSource, enqueue};
use daemon::runner::{TurnOutcome, TurnResult};
use daemon::team::{SharedAttachments, Team, TeamDeps};
use protocol::Event;
use protocol::attachments::AttachmentCompanion;
use protocol::model::{ExchangePostData, FileData, Message, MessageKind};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// `done` in this file: its text is "Done".
fn done() -> TurnResult {
    TurnResult {
        outcome: TurnOutcome::Done,
        text: Some("Done".to_owned()),
        text_message_id: None,
        error: None,
    }
}

/// The 1x1 PNG the TypeScript test writes as `chart.png`.
fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aR5sAAAAASUVORK5CYII=")
        .unwrap()
}

/// `writer` and `asker` in the TypeScript. Each may message the other, and Writer's folder is its home.
struct Bots {
    writer: StoredBot,
    asker: StoredBot,
}

fn bots(fx: &Fixture) -> Bots {
    let writer = fx.bot("Writer", "blue", "circle", "", "codex", "synthetic");
    let asker = fx.bot("Asker", "blue", "circle", "", "codex", "synthetic");
    let asker = fx.set_can_message(&asker.id, &[writer.id.as_str()]);
    let writer = fx.set_can_message(&writer.id, &[asker.id.as_str()]);
    use_folder(fx, &writer.id, Path::new(&writer.home_folder));
    Bots { writer, asker }
}

/// `effectiveFolder: () => folder`, for Writer.
fn use_folder(fx: &Fixture, bot_id: &str, folder: &Path) {
    fx.effective
        .lock()
        .unwrap()
        .insert(bot_id.to_owned(), folder.to_string_lossy().into_owned());
}

/// `history(db, conversationId, undefined, limit)`: the newest `limit` messages, oldest first.
fn history_of(fx: &Fixture, conversation_id: &str, limit: u32) -> Vec<Message> {
    fx.db(|db| history(db, conversation_id, None, limit).unwrap())
}

/// `service.metadata({ messageId, attachmentIndex })` as a lookup: the attachment a message links at `index`.
fn link_of(fx: &Fixture, message_id: &str, index: u64) -> Option<String> {
    fx.db(|db| {
        db.query_row(
            "SELECT attachment_id FROM message_attachments WHERE message_id = ?1 AND attachment_index = ?2",
            params![message_id, index as i64],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .unwrap()
    })
}

/// A row of `attachments`. Its bytes are in `shared-files\attachments\<storage_name>`.
struct StoredAttachment {
    storage_name: String,
    content_type: String,
    sha256: String,
}

fn stored(fx: &Fixture, attachment_id: &str) -> StoredAttachment {
    fx.db(|db| {
        db.query_row(
            "SELECT storage_name, content_type, sha256 FROM attachments WHERE id = ?1",
            [attachment_id],
            |row| {
                Ok(StoredAttachment {
                    storage_name: row.get(0)?,
                    content_type: row.get(1)?,
                    sha256: row.get(2)?,
                })
            },
        )
        .unwrap()
    })
}

fn attachment_exists(fx: &Fixture, attachment_id: &str) -> bool {
    fx.db(|db| {
        db.query_row(
            "SELECT COUNT(*) FROM attachments WHERE id = ?1",
            [attachment_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            > 0
    })
}

/// The bytes `open` and `read` would give: the stored copy, checked against the row's sha256.
fn stored_bytes(fx: &Fixture, attachment_id: &str) -> Vec<u8> {
    let row = stored(fx, attachment_id);
    let bytes = fs::read(
        fx.shared_files()
            .join("attachments")
            .join(&row.storage_name),
    )
    .unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), row.sha256);
    bytes
}

/// The `FileData` of a file card.
fn file_data(message: &Message) -> FileData {
    serde_json::from_value(Value::Object(
        message.data.clone().expect("a file card has data"),
    ))
    .unwrap()
}

fn name_is(message: &Message, name: &str) -> bool {
    message.kind == MessageKind::File
        && message
            .data
            .as_ref()
            .and_then(|data| data.get("name"))
            .and_then(Value::as_str)
            == Some(name)
}

fn has_files(message: &Message) -> bool {
    message
        .data
        .as_ref()
        .and_then(|data| data.get("files"))
        .is_some_and(Value::is_array)
}

fn last_delivered(fx: &Fixture) -> WorkItem {
    recorded(&fx.delivered)
        .last()
        .cloned()
        .expect("something was delivered")
}

fn set_current(fx: &Fixture, bot_id: &str, item: WorkItem) {
    fx.current.lock().unwrap().insert(bot_id.to_owned(), item);
}

/// A task's work item (`enqueue` with `source: "task"`), in the attempt "attempt".
fn task_item(fx: &Fixture, bot_id: &str, revision: i64, context_key: &str) -> WorkItem {
    fx.db(|db| {
        enqueue(
            db,
            &NewWork {
                task_attempt_id: Some("attempt".to_owned()),
                task_revision: Some(revision),
                execution_context_key: Some(context_key.to_owned()),
                ..NewWork::new(bot_id, WorkSource::Task, "task")
            },
        )
        .unwrap()
    })
}

#[test]
fn publishes_linked_immutable_bytes_before_announcing_a_delivered_file() {
    let mut fx = fixture_with(true);
    // TypeScript's broadcast looks each announced file's metadata up at the moment of the announcement, and throws if the
    // link isn't there yet. The same check here reads the links the attachments have made so far (the broadcast must not
    // take the host's database lock).
    let published: Recorded<String> = Arc::default();
    {
        let linked = fx.attachments.as_ref().unwrap().linked.clone();
        let (events, logs, seen) = (fx.events.clone(), fx.logs.clone(), published.clone());
        fx.team = Arc::new(Team::new(TeamDeps {
            broadcast: Arc::new(move |event: Event| {
                if let Event::MessageAdded(message) = &event
                    && message.kind == MessageKind::File
                {
                    let link = recorded(&linked)
                        .iter()
                        .find(|(id, index, _)| *id == message.id && *index == 0)
                        .map(|(_, _, attachment)| attachment.clone());
                    recorded(&seen)
                        .push(link.unwrap_or_else(|| format!("{} has no link yet", message.id)));
                }
                recorded(&events).push(event);
                Ok(())
            }),
            log: Arc::new(move |context: &str, error: &str| {
                recorded(&logs).push((context.to_owned(), error.to_owned()));
            }),
            shared_files: fx.shared_files(),
            attachments: fx
                .attachments
                .clone()
                .map(|shared| shared as Arc<dyn SharedAttachments>),
        }));
    }
    let b = bots(&fx);
    let folder = PathBuf::from(&b.writer.home_folder);
    fs::write(folder.join("answer.md"), "# Answer").unwrap();
    fx.run_tool("share_file", &b.writer.id, json!({ "path": "answer.md" }))
        .unwrap();
    let message = history_of(&fx, &b.writer.id, 200).remove(0);
    let metadata_id = link_of(&fx, &message.id, 0).expect("the card links its file");
    assert_eq!(*recorded(&published), std::slice::from_ref(&metadata_id));
    let announced: Vec<String> = fx
        .events_named("message.added")
        .iter()
        .filter(|event| event["kind"] == "file")
        .map(|event| event["data"]["attachmentId"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(announced, vec![metadata_id.clone()]);
    let card = file_data(&message);
    assert_eq!(card.attachment_id.as_deref(), Some(metadata_id.as_str()));
    assert_eq!(card.content_type.as_deref(), Some("text/markdown"));
    fs::write(folder.join("answer.md"), "replacement").unwrap();
    assert_eq!(
        String::from_utf8(stored_bytes(&fx, &metadata_id)).unwrap(),
        "# Answer"
    );
}

#[test]
fn shares_from_the_validated_task_folder_and_refuses_a_sibling_managed_folder() {
    let fx = fixture_with(true);
    let b = bots(&fx);
    let folder = fx.root.path().join("workspaces").join("owned");
    fs::create_dir_all(&folder).unwrap();
    let sibling = fx.root.path().join("workspaces").join("other");
    fs::create_dir(&sibling).unwrap();
    fs::write(sibling.join("private.md"), "private").unwrap();
    use_folder(&fx, &b.writer.id, &folder);
    set_current(
        &fx,
        &b.writer.id,
        task_item(&fx, &b.writer.id, 1, "context"),
    );
    fs::write(folder.join("answer.md"), "task result").unwrap();
    assert_eq!(
        fx.run_tool("share_file", &b.writer.id, json!({ "path": "answer.md" })),
        Ok("Shared answer.md.".to_owned())
    );
    assert!(
        fx.run_tool(
            "share_file",
            &b.writer.id,
            json!({ "path": sibling.join("private.md").to_string_lossy() }),
        )
        .is_err()
    );
}

#[test]
fn preserves_task_reply_context_and_attachment_identity_in_exchange_room_copies() {
    let fx = fixture_with(true);
    let b = bots(&fx);
    let folder = PathBuf::from(&b.writer.home_folder);
    set_current(
        &fx,
        &b.asker.id,
        task_item(&fx, &b.asker.id, 3, "exact-context"),
    );
    fx.run_tool(
        "message_bot",
        &b.asker.id,
        json!({ "bot": "Writer", "message": "Help" }),
    )
    .unwrap();
    let request = last_delivered(&fx);
    set_current(&fx, &b.writer.id, request.clone());
    fs::write(folder.join("answer.md"), "answer").unwrap();
    fx.run_tool("share_file", &b.writer.id, json!({ "path": "answer.md" }))
        .unwrap();
    let original = history_of(&fx, &request.conversation_id, 200)
        .into_iter()
        .find(|entry| entry.kind == MessageKind::File)
        .unwrap();
    fx.team_call(|team, host| team.turn_ended(host, &request, &done()))
        .unwrap();
    let reply = last_delivered(&fx);
    assert_eq!(reply.bot_id, b.asker.id);
    assert_eq!(reply.source, WorkSource::BotReply);
    assert_eq!(reply.task_attempt_id.as_deref(), Some("attempt"));
    assert_eq!(reply.task_revision, Some(3));
    assert_eq!(
        reply.execution_context_key.as_deref(),
        Some("exact-context")
    );
    let room = history_of(&fx, "team", 200)
        .into_iter()
        .find(has_files)
        .unwrap();
    assert_eq!(
        link_of(&fx, &room.id, 0),
        file_data(&original).attachment_id
    );
}

#[test]
fn publishes_explicit_companion_ownership_atomically_and_revokes_it_with_its_parent_message() {
    let fx = fixture_with(true);
    let b = bots(&fx);
    let folder = PathBuf::from(&b.writer.home_folder);
    fs::write(
        folder.join("answer.md"),
        "# Answer\n![Chart](images/chart.png)",
    )
    .unwrap();
    fs::write(folder.join("chart.png"), png()).unwrap();
    fx.run_tool(
        "share_file",
        &b.writer.id,
        json!({
            "path": "answer.md",
            "companions": [{ "name": "./images/chart.png", "path": "chart.png" }],
        }),
    )
    .unwrap();
    let message = history_of(&fx, &b.writer.id, 200)
        .into_iter()
        .find(|item| name_is(item, "answer.md"))
        .unwrap();
    let parent_id = file_data(&message).attachment_id.unwrap();
    let companions = file_data(&message).companions.unwrap();
    let companion = companions.as_slice()[0].clone();
    assert_eq!(companion.name.as_str(), "images/chart.png");
    assert_eq!(companion.attachment_index, 1);
    // `JSON.stringify(companion)` never names the folder, in either its plain or its escaped spelling.
    let root = fx.root.path().to_string_lossy().into_owned();
    let text = serde_json::to_string(&companion).unwrap();
    assert!(!text.contains(&root));
    assert!(!text.contains(&root.replace('\\', "\\\\")));
    assert_eq!(
        stored(&fx, &companion.attachment_id).content_type,
        "image/png"
    );
    assert_eq!(stored_bytes(&fx, &companion.attachment_id), png());
    // The lookups `metadata` would refuse as unavailable: another index, or the parent's id at the companion's index.
    assert_eq!(link_of(&fx, &message.id, 42), None);
    let linked = link_of(&fx, &message.id, companion.attachment_index);
    assert_eq!(linked, Some(companion.attachment_id.clone()));
    assert_ne!(linked, Some(parent_id));
    // Deleting the parent message revokes the companion with it.
    let companion_file = stored(&fx, &companion.attachment_id).storage_name;
    fx.db(|db| {
        db.execute("DELETE FROM messages WHERE id = ?1", [&message.id])
            .unwrap();
    });
    let shared = fx.attachments.as_ref().unwrap();
    fx.db(|db| shared.cleanup(db).unwrap());
    assert!(!attachment_exists(&fx, &companion.attachment_id));
    assert!(
        !fx.shared_files()
            .join("attachments")
            .join(companion_file)
            .exists()
    );
}

#[test]
fn remaps_room_companion_indexes_without_changing_ids_across_delivery_and_restart() {
    let fx = fixture_with(true);
    let b = bots(&fx);
    let folder = PathBuf::from(&b.writer.home_folder);
    fx.run_tool(
        "message_bot",
        &b.asker.id,
        json!({ "bot": "Writer", "message": "Help" }),
    )
    .unwrap();
    let request = last_delivered(&fx);
    set_current(&fx, &b.writer.id, request.clone());
    fs::write(folder.join("answer.md"), "![Chart](chart.png)").unwrap();
    fs::write(folder.join("chart.png"), png()).unwrap();
    fx.run_tool(
        "share_file",
        &b.writer.id,
        json!({
            "path": "answer.md",
            "companions": [{ "name": "chart.png", "path": "chart.png" }],
        }),
    )
    .unwrap();
    let original = history_of(&fx, &request.conversation_id, 200)
        .into_iter()
        .find(|entry| name_is(entry, "answer.md"))
        .unwrap();
    let companion = file_data(&original).companions.unwrap().as_slice()[0].clone();
    fx.team_call(|team, host| team.turn_ended(host, &request, &done()))
        .unwrap();
    let room = history_of(&fx, "team", 200)
        .into_iter()
        .find(has_files)
        .unwrap();
    let post: ExchangePostData =
        serde_json::from_value(Value::Object(room.data.clone().unwrap())).unwrap();
    let files = post.files.unwrap();
    assert_eq!(files.len(), 2);
    let room_companion = files[0].companions.as_ref().unwrap().as_slice()[0].clone();
    assert_eq!(
        room_companion,
        AttachmentCompanion {
            attachment_index: 2,
            ..companion.clone()
        }
    );
    assert_eq!(
        link_of(&fx, &room.id, room_companion.attachment_index),
        Some(companion.attachment_id.clone())
    );
    assert_eq!(
        link_of(&fx, &room.id, 1),
        Some(companion.attachment_id.clone())
    );
    // A restart: a new attachments service over the same folder, which sweeps what no message links.
    let restarted = DbAttachments {
        root: fx.shared_files().join("attachments"),
        host_id: "host".to_owned(),
        linked: Arc::default(),
    };
    fx.db(|db| restarted.cleanup(db).unwrap());
    assert_eq!(
        link_of(&fx, &room.id, room_companion.attachment_index),
        Some(companion.attachment_id.clone())
    );
}

#[test]
fn refuses_companion_escape_duplicate_names_and_active_formats_without_publishing_a_partial_bundle()
{
    let fx = fixture_with(true);
    let b = bots(&fx);
    let folder = PathBuf::from(&b.writer.home_folder);
    fs::write(folder.join("answer.md"), "# Answer").unwrap();
    fs::write(folder.join("chart.png"), png()).unwrap();
    fs::write(folder.join("active.svg"), "<svg/>").unwrap();
    let outside = fx.root.path().join("outside.png");
    fs::write(&outside, png()).unwrap();
    let cases = [
        json!([{ "name": "outside.png", "path": outside.to_string_lossy() }]),
        json!([{ "name": "../chart.png", "path": "chart.png" }]),
        json!([
            { "name": "chart.png", "path": "chart.png" },
            { "name": "./chart.png", "path": "chart.png" },
        ]),
        json!([{ "name": "active.svg", "path": "active.svg" }]),
    ];
    for companions in cases {
        assert!(
            fx.run_tool(
                "share_file",
                &b.writer.id,
                json!({ "path": "answer.md", "companions": companions.clone() }),
            )
            .is_err(),
            "{companions}"
        );
        assert!(
            history_of(&fx, &b.writer.id, 200).is_empty(),
            "{companions}"
        );
        assert_eq!(
            fx.count("SELECT COUNT(*) FROM attachments"),
            0,
            "{companions}"
        );
    }
}
