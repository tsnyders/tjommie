//! Port of `packages/daemon/src/attachments.test.ts`, plus the device ownership checks (M6): a paired device can't use
//! another device's transfer, another connection's, or a file no message shares. Messages are inserted directly until
//! T3.5's chat port lands (`addMessage`).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use daemon::attachments::{Attachments, Backfill, attachment_content_type};
use daemon::bots::{StoredBot, create_bot};
use daemon::context::Db;
use daemon::devices_store::{add_device, revoke_device};
use daemon::server::{Handlers, Principal, RequestContext, RpcError, Server, ServerOptions};
use daemon::spawn;
use protocol::ErrorCode;
use protocol::attachments::{
    ATTACHMENT_CHUNK_BYTES, AttachmentInfo, AttachmentRead, AttachmentRef, MAX_ATTACHMENT_BYTES,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

const CHUNK: usize = ATTACHMENT_CHUNK_BYTES as usize;

struct Fixture {
    root: PathBuf,
    _dir: tempfile::TempDir,
    db: Db,
    service: Arc<Attachments>,
    bot: StoredBot,
    context: RequestContext,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    fs::create_dir_all(root.join("data")).unwrap();
    fs::create_dir_all(root.join("shared-files").join("share-fixture")).unwrap();
    let connection = daemon::db::open(&root.join("data").join("tjommie.db")).unwrap();
    let input = serde_json::from_value(json!({
        "name": "Files", "color": "blue", "shape": "circle", "instructions": "", "provider": "codex", "model": "synthetic"
    }))
    .unwrap();
    let bot = create_bot(&connection, &root.join("bots"), &input).unwrap();
    let db: Db = Arc::new(std::sync::Mutex::new(connection));
    let service = Arc::new(Attachments::new(db.clone(), &root, "fixture-host").unwrap());
    let context = local("local-connection");
    Fixture {
        root,
        _dir: dir,
        db,
        service,
        bot,
        context,
    }
}

fn local(connection: &str) -> RequestContext {
    RequestContext {
        connection_id: connection.into(),
        principal: Principal::Local,
        signal: CancellationToken::new(),
    }
}

fn device(connection: &str, device_id: &str) -> RequestContext {
    RequestContext {
        connection_id: connection.into(),
        principal: Principal::Device {
            device_id: device_id.into(),
        },
        signal: CancellationToken::new(),
    }
}

/// `addMessage` for the columns attachments reads.
fn add_message(db: &Db, conversation: &str, kind: &str, data: &Value) -> String {
    let id = daemon::ids::new_id();
    db.lock()
        .unwrap()
        .execute(
            "INSERT INTO messages(id,conversation_id,author,kind,body,data,status,created_at,seq)
             VALUES(?1,?2,?2,?3,'file',?4,'done',?5,(SELECT COALESCE(MAX(seq),0)+1 FROM messages))",
            [
                &id,
                conversation,
                kind,
                &data.to_string(),
                &daemon::ids::now_iso(),
            ],
        )
        .unwrap();
    id
}

struct Published {
    source: PathBuf,
    info: AttachmentInfo,
    reference: AttachmentRef,
}

impl Fixture {
    fn publish(&self, bytes: &[u8], name: &str) -> Published {
        let source = self
            .root
            .join("shared-files")
            .join("share-fixture")
            .join(name);
        fs::write(&source, bytes).unwrap();
        let info = self.service.register_snapshot(&source, name).unwrap();
        let data =
            json!({ "path": source, "name": name, "size": bytes.len(), "attachmentId": info.id });
        let message = add_message(&self.db, &self.bot.id, "file", &data);
        self.service.link(&message, 0, &info.id).unwrap();
        let reference = AttachmentRef {
            message_id: message,
            attachment_index: 0,
            attachment_id: Some(info.id.clone()),
        };
        Published {
            source,
            info,
            reference,
        }
    }

    fn stored(&self, info: &AttachmentInfo) -> PathBuf {
        self.root
            .join("shared-files")
            .join("attachments")
            .join(format!("{}.bin", info.id))
    }

    fn read(
        &self,
        transfer: &str,
        offset: u64,
        length: u32,
        context: &RequestContext,
    ) -> Result<protocol::attachments::AttachmentChunk, RpcError> {
        let input = AttachmentRead {
            transfer_id: transfer.into(),
            offset,
            length,
        };
        self.service.read(&input, Some(context))
    }
}

fn code<T: std::fmt::Debug>(result: Result<T, RpcError>) -> ErrorCode {
    result.unwrap_err().code
}

fn bytes(chunk: &protocol::attachments::AttachmentChunk) -> Vec<u8> {
    STANDARD.decode(&chunk.bytes).unwrap()
}

/// The 8.3 spelling of an existing path, if the volume makes them.
fn short_name(path: &Path) -> Option<PathBuf> {
    use windows::Win32::Storage::FileSystem::GetShortPathNameW;
    use windows::core::HSTRING;
    let mut buffer = [0u16; 1024];
    // SAFETY: the call writes at most the buffer's length into it.
    let length =
        unsafe { GetShortPathNameW(&HSTRING::from(path.as_os_str()), Some(&mut buffer)) } as usize;
    if length == 0 || length >= buffer.len() {
        return None;
    }
    let short = PathBuf::from(String::from_utf16(&buffer[..length]).ok()?);
    (short != path).then_some(short)
}

fn junction(link: &Path, target: &Path) {
    let made = spawn::command("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stdout)
    );
}

#[test]
fn accepts_a_windows_short_path_alias_without_relaxing_linked_path_rejection() {
    let f = fixture();
    let Some(alias) = short_name(&f.root) else {
        eprintln!("skipped: this volume makes no 8.3 names");
        return;
    };
    let published = f.publish(b"Short path snapshot", "answer.md");
    let via_alias = alias
        .join("shared-files")
        .join("share-fixture")
        .join("answer.md");
    let again = f
        .service
        .register_snapshot(&via_alias, "answer.md")
        .unwrap();
    assert_eq!(
        (again.sha256, again.size),
        (published.info.sha256, published.info.size)
    );
    let aliased_home = alias.join(Path::new(&f.bot.home_folder).strip_prefix(&f.root).unwrap());
    f.db.lock()
        .unwrap()
        .execute(
            "UPDATE bots SET home_folder=?1 WHERE id=?2",
            [aliased_home.to_str().unwrap(), &f.bot.id],
        )
        .unwrap();
    let inbox = aliased_home.join("inbox").join("legacy.md");
    fs::write(&inbox, "legacy").unwrap();
    add_message(
        &f.db,
        &f.bot.id,
        "file",
        &json!({ "path": inbox, "name": "legacy.md", "size": 6 }),
    );
    assert_eq!(
        f.service.backfill_legacy().unwrap(),
        Backfill {
            registered: 1,
            unavailable: 0
        }
    );
}

#[test]
fn serves_the_registered_copy_after_the_source_is_changed_or_deleted_and_returns_bounded_exact_bytes()
 {
    let f = fixture();
    let published = f.publish(&vec![97; CHUNK + 3], "answer.md");
    fs::write(&published.source, "replacement").unwrap();
    fs::remove_file(&published.source).unwrap();
    assert_eq!(
        f.service
            .metadata(&published.reference, Some(&f.context))
            .unwrap(),
        published.info
    );
    let transfer = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    let first = f
        .read(&transfer.transfer_id, 0, ATTACHMENT_CHUNK_BYTES, &f.context)
        .unwrap();
    assert_eq!(bytes(&first).len(), CHUNK);
    assert!(!first.eof);
    let last = f
        .read(
            &transfer.transfer_id,
            CHUNK as u64,
            ATTACHMENT_CHUNK_BYTES,
            &f.context,
        )
        .unwrap();
    assert_eq!(bytes(&last), b"aaa");
    assert!(last.eof);
    assert_eq!(last.revision, published.info.sha256);
    let shown = serde_json::to_string(&transfer).unwrap();
    assert!(!shown.contains(&f.root.to_string_lossy().replace('\\', "\\\\")));
}

#[test]
fn handles_a_zero_byte_delivered_file_and_eof_without_a_fake_byte() {
    let f = fixture();
    let published = f.publish(b"", "empty.md");
    let transfer = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    let chunk = f.read(&transfer.transfer_id, 0, 1, &f.context).unwrap();
    assert_eq!(
        (chunk.offset, chunk.bytes.as_str(), chunk.eof),
        (0, "", true)
    );
}

async fn server(service: &Arc<Attachments>) -> Server {
    Server::start(ServerOptions {
        port: 0,
        token: "synthetic-token".into(),
        handlers: service.handlers(Handlers::default()),
        allowed_origins: Vec::new(),
        defer_requests: false,
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn requires_trusted_context_and_does_not_accept_a_path_or_identity_in_params() {
    let f = fixture();
    let published = f.publish(b"immutable snapshot", "answer.md");
    let server = server(&f.service).await;
    let reference = serde_json::to_value(&published.reference).unwrap();
    for method in ["attachments.metadata", "attachments.open"] {
        let refused = server
            .dispatch(method, reference.clone(), None)
            .await
            .unwrap_err();
        assert_eq!(refused.code, ErrorCode::Forbidden, "{method}");
        assert!(refused.message.contains("authorized"));
    }
    let mut with_path = reference.clone();
    with_path["path"] = json!("C:\\private.txt");
    let refused = server
        .dispatch("attachments.metadata", with_path, Some(f.context.clone()))
        .await;
    assert_eq!(code(refused), ErrorCode::BadParams);
    let mut other = published.reference.clone();
    other.attachment_id = Some("another-id".into());
    let refused = f.service.metadata(&other, Some(&f.context)).unwrap_err();
    assert!(
        refused.message.contains("unavailable"),
        "{}",
        refused.message
    );
    f.db.lock()
        .unwrap()
        .execute(
            "UPDATE attachments SET host_id='another-host' WHERE id=?1",
            [&published.info.id],
        )
        .unwrap();
    assert!(
        f.service
            .metadata(&published.reference, Some(&f.context))
            .unwrap_err()
            .message
            .contains("unavailable")
    );
    server.close().await;
}

#[tokio::test]
async fn answers_every_attachment_method_within_the_manifest() {
    let f = fixture();
    let published = f.publish(b"abcd", "answer.md");
    let server = server(&f.service).await;
    let reference = serde_json::to_value(&published.reference).unwrap();
    let context = Some(f.context.clone());
    let metadata = server
        .dispatch("attachments.metadata", reference.clone(), context.clone())
        .await
        .unwrap();
    protocol::assert_valid("attachments.metadata", &metadata);
    let opened = server
        .dispatch("attachments.open", reference, context.clone())
        .await
        .unwrap();
    protocol::assert_valid("attachments.open", &opened);
    let transfer = opened["transferId"].clone();
    let read = server
        .dispatch(
            "attachments.read",
            json!({ "transferId": transfer, "offset": 0, "length": 4 }),
            context.clone(),
        )
        .await
        .unwrap();
    protocol::assert_valid("attachments.read", &read);
    assert_eq!(read["bytes"], STANDARD.encode("abcd"));
    for (offset, length) in [
        (json!(-1), json!(1)),
        (json!(0.5), json!(1)),
        (json!(0), json!(0)),
        (json!(0), json!(CHUNK + 1)),
    ] {
        let refused = server
            .dispatch(
                "attachments.read",
                json!({ "transferId": transfer, "offset": offset, "length": length }),
                context.clone(),
            )
            .await;
        assert_eq!(code(refused), ErrorCode::BadParams);
    }
    let closed = server
        .dispatch(
            "attachments.close",
            json!({ "transferId": transfer }),
            context,
        )
        .await
        .unwrap();
    protocol::assert_valid("attachments.close", &closed);
    server.close().await;
}

#[test]
fn limits_parallel_opens_serializes_reads_and_isolates_connection_and_principal_handles() {
    let f = fixture();
    let published = f.publish(&vec![1; 16 * 1024 * 1024], "large.bin");
    let first = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    let second = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    assert_eq!(
        code(f.service.open(&published.reference, Some(&f.context))),
        ErrorCode::Busy
    );
    std::thread::scope(|scope| {
        // The first read verifies 16 MiB before releasing bytes; a second read meanwhile is refused, not queued.
        let reading =
            scope.spawn(|| f.read(&first.transfer_id, 0, ATTACHMENT_CHUNK_BYTES, &f.context));
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            code(f.read(&first.transfer_id, 0, 1, &f.context)),
            ErrorCode::Busy
        );
        reading.join().unwrap().unwrap();
    });
    let other = local("new-connection");
    assert_eq!(
        code(f.read(&first.transfer_id, 0, 1, &other)),
        ErrorCode::Forbidden
    );
    assert_eq!(
        code(f.service.close_transfer(&first.transfer_id, Some(&other))),
        ErrorCode::Forbidden
    );
    f.service
        .close_transfer(&second.transfer_id, Some(&f.context))
        .unwrap();
    f.service
        .close_transfer(&second.transfer_id, Some(&f.context))
        .unwrap();
}

#[test]
fn rejects_every_malformed_range_without_ending_a_valid_transfer() {
    let f = fixture();
    let published = f.publish(b"abcd", "answer.md");
    let transfer = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    // Negative and fractional offsets, a zero length and an oversized one can't reach the service: decoding refuses
    // them (answers_every_attachment_method_within_the_manifest). These pass decoding and fail the range check.
    for (offset, length) in [(5, 1), (0, 0), (0, ATTACHMENT_CHUNK_BYTES + 1)] {
        assert_eq!(
            code(f.read(&transfer.transfer_id, offset, length, &f.context)),
            ErrorCode::BadParams,
            "{offset} {length}"
        );
    }
    assert_eq!(
        f.read(&transfer.transfer_id, 4, 1, &f.context)
            .unwrap()
            .bytes,
        ""
    );
}

#[test]
fn aborts_on_disconnect_and_checks_device_revocation_on_each_operation() {
    let f = fixture();
    let published = f.publish(b"immutable snapshot", "answer.md");
    let first = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    f.context.signal.cancel();
    assert_eq!(
        code(f.read(&first.transfer_id, 0, 1, &f.context)),
        ErrorCode::Forbidden
    );
    let phone = add_device(
        &f.db.lock().unwrap(),
        &URL_SAFE_NO_PAD.encode([7; 32]),
        "Synthetic device",
    )
    .unwrap();
    let paired = device("paired", &phone.id);
    let remote = f.service.open(&published.reference, Some(&paired)).unwrap();
    revoke_device(&f.db.lock().unwrap(), &phone.id).unwrap();
    assert_eq!(
        code(f.read(&remote.transfer_id, 0, 1, &paired)),
        ErrorCode::Forbidden
    );
    let refused = f
        .service
        .metadata(&published.reference, Some(&paired))
        .unwrap_err();
    assert!(
        refused.message.contains("authorized"),
        "{}",
        refused.message
    );
}

#[test]
fn a_device_cannot_use_another_devices_transfer_or_a_file_no_message_shares() {
    let f = fixture();
    let published = f.publish(b"shared with the conversation", "answer.md");
    let (one, two) = {
        let db = f.db.lock().unwrap();
        (
            add_device(&db, &URL_SAFE_NO_PAD.encode([1; 32]), "Phone one").unwrap(),
            add_device(&db, &URL_SAFE_NO_PAD.encode([2; 32]), "Phone two").unwrap(),
        )
    };
    let first = device("first-phone", &one.id);
    let second = device("second-phone", &two.id);
    let transfer = f.service.open(&published.reference, Some(&first)).unwrap();
    // Another device, the same device on another connection, and this PC all lack the handle's owner and connection.
    for other in [
        &second,
        &device("first-phone-again", &one.id),
        &local("first-phone"),
    ] {
        assert_eq!(
            code(f.read(&transfer.transfer_id, 0, 1, other)),
            ErrorCode::Forbidden
        );
        assert_eq!(
            code(f.service.close_transfer(&transfer.transfer_id, Some(other))),
            ErrorCode::Forbidden
        );
    }
    assert_eq!(
        bytes(&f.read(&transfer.transfer_id, 0, 6, &first).unwrap()),
        b"shared"
    );
    // Two transfers per device, whichever connection opened them.
    f.service
        .open(
            &published.reference,
            Some(&device("first-phone-b", &one.id)),
        )
        .unwrap();
    assert_eq!(
        code(f.service.open(&published.reference, Some(&first))),
        ErrorCode::Busy
    );
    f.service.open(&published.reference, Some(&second)).unwrap();

    // A registered file no message links is unreachable by its id, by a message's other index, or by another message.
    let source = f
        .root
        .join("shared-files")
        .join("share-fixture")
        .join("private.md");
    fs::write(&source, "not shared").unwrap();
    let unshared = f.service.register_snapshot(&source, "private.md").unwrap();
    let mut by_id = published.reference.clone();
    by_id.attachment_id = Some(unshared.id.clone());
    let mut by_index = published.reference.clone();
    by_index.attachment_index = 1;
    by_index.attachment_id = None;
    let other_message = add_message(&f.db, &f.bot.id, "text", &json!({ "text": "no files" }));
    let by_message = AttachmentRef {
        message_id: other_message,
        attachment_index: 0,
        attachment_id: Some(unshared.id),
    };
    for reference in [by_id, by_index, by_message] {
        let refused = f.service.metadata(&reference, Some(&second)).unwrap_err();
        assert_eq!(refused.code, ErrorCode::Unavailable, "{reference:?}");
        assert_eq!(
            code(f.service.open(&reference, Some(&second))),
            ErrorCode::Unavailable
        );
    }
}

#[test]
fn invalidates_reads_when_message_ownership_disappears_and_retains_leased_bytes_until_close() {
    let f = fixture();
    let published = f.publish(b"immutable snapshot", "answer.md");
    let transfer = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    let file = f.stored(&published.info);
    f.db.lock()
        .unwrap()
        .execute(
            "DELETE FROM messages WHERE id=?1",
            [&published.reference.message_id],
        )
        .unwrap();
    f.service.cleanup().unwrap();
    assert!(file.exists());
    assert_eq!(
        code(f.read(&transfer.transfer_id, 0, 1, &f.context)),
        ErrorCode::Unavailable
    );
    f.service
        .close_transfer(&transfer.transfer_id, Some(&f.context))
        .unwrap();
    f.service.cleanup().unwrap();
    assert!(!file.exists());
}

#[test]
fn detects_corrupt_content_before_releasing_bytes_and_changed_content_before_subsequent_reads() {
    let f = fixture();
    let published = f.publish(b"first", "answer.md");
    let file = f.stored(&published.info);
    fs::write(&file, "other").unwrap();
    let corrupt = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    assert_eq!(
        code(f.read(&corrupt.transfer_id, 0, 1, &f.context)),
        ErrorCode::Corrupt
    );
    fs::write(&file, "first").unwrap();
    let transfer = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    fs::write(&file, "longer").unwrap();
    assert_eq!(
        code(f.read(&transfer.transfer_id, 0, 1, &f.context)),
        ErrorCode::Corrupt
    );
}

#[test]
fn cancels_an_in_progress_first_read_through_the_returned_transfer_handle() {
    let f = fixture();
    let published = f.publish(&vec![7; 32 * 1024 * 1024], "large.bin");
    let transfer = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    std::thread::scope(|scope| {
        let reading = scope.spawn(|| f.read(&transfer.transfer_id, 0, 1, &f.context));
        std::thread::sleep(Duration::from_millis(20));
        f.service
            .close_transfer(&transfer.transfer_id, Some(&f.context))
            .unwrap();
        assert_eq!(code(reading.join().unwrap()), ErrorCode::Forbidden);
    });
    let again = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    assert_eq!(Some(again.attachment.id), published.reference.attachment_id);
}

#[test]
fn adopts_only_known_inbox_or_staging_legacy_files_and_preserves_the_original_inbox_copy() {
    let f = fixture();
    let inbox = Path::new(&f.bot.home_folder).join("inbox").join("old.md");
    fs::write(&inbox, "legacy").unwrap();
    let outside = f.root.join("project.md");
    fs::write(&outside, "outside").unwrap();
    let missing = Path::new(&f.bot.home_folder)
        .join("inbox")
        .join("missing.md");
    let ids: Vec<String> = [&inbox, &outside, &missing]
        .iter()
        .map(|path| {
            add_message(
                &f.db,
                &f.bot.id,
                "file",
                &json!({ "path": path, "name": "old.md", "size": 6 }),
            )
        })
        .collect();
    assert_eq!(
        f.service.backfill_legacy().unwrap(),
        Backfill {
            registered: 1,
            unavailable: 2
        }
    );
    let first = AttachmentRef {
        message_id: ids[0].clone(),
        attachment_index: 0,
        attachment_id: None,
    };
    assert_eq!(
        f.service.metadata(&first, Some(&f.context)).unwrap().sha256,
        format!("{:x}", Sha256::digest(b"legacy"))
    );
    assert_eq!(fs::read_to_string(&inbox).unwrap(), "legacy");
    assert_eq!(
        f.service.backfill_legacy().unwrap(),
        Backfill {
            registered: 0,
            unavailable: 2
        }
    );
    for id in &ids[1..] {
        let reference = AttachmentRef {
            message_id: id.clone(),
            attachment_index: 0,
            attachment_id: None,
        };
        assert!(
            f.service
                .metadata(&reference, Some(&f.context))
                .unwrap_err()
                .message
                .contains("unavailable")
        );
    }
}

#[test]
fn refuses_linked_legacy_parents_and_oversized_snapshots_without_importing_outside_data() {
    let f = fixture();
    let outside = f.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("private.md"), "outside").unwrap();
    let linked = Path::new(&f.bot.home_folder).join("inbox").join("linked");
    junction(&linked, &outside);
    let message = add_message(
        &f.db,
        &f.bot.id,
        "file",
        &json!({ "path": linked.join("private.md"), "name": "private.md", "size": 7 }),
    );
    let reference = AttachmentRef {
        message_id: message,
        attachment_index: 0,
        attachment_id: None,
    };
    assert!(
        f.service
            .metadata(&reference, Some(&f.context))
            .unwrap_err()
            .message
            .contains("unavailable")
    );
    let large = f
        .root
        .join("shared-files")
        .join("share-fixture")
        .join("large.bin");
    fs::File::create_new(&large)
        .unwrap()
        .set_len(MAX_ATTACHMENT_BYTES + 1)
        .unwrap();
    let refused = f
        .service
        .register_snapshot(&large, "large.bin")
        .unwrap_err();
    assert!(refused.message.contains("too large"), "{}", refused.message);
}

#[test]
fn rejects_storage_traversal_and_a_legacy_file_whose_recorded_size_no_longer_matches() {
    let f = fixture();
    let published = f.publish(b"immutable snapshot", "answer.md");
    f.db.lock()
        .unwrap()
        .execute(
            "UPDATE attachments SET storage_name='../../private.txt' WHERE id=?1",
            [&published.info.id],
        )
        .unwrap();
    let refused = f
        .service
        .metadata(&published.reference, Some(&f.context))
        .unwrap_err();
    assert!(
        refused.message.contains("record is invalid"),
        "{}",
        refused.message
    );
    let file = Path::new(&f.bot.home_folder)
        .join("inbox")
        .join("changed.md");
    fs::write(&file, "modified after delivery").unwrap();
    let message = add_message(
        &f.db,
        &f.bot.id,
        "file",
        &json!({ "path": file, "name": "changed.md", "size": 3 }),
    );
    let reference = AttachmentRef {
        message_id: message,
        attachment_index: 0,
        attachment_id: None,
    };
    assert!(
        f.service
            .metadata(&reference, Some(&f.context))
            .unwrap_err()
            .message
            .contains("unavailable")
    );
}

#[tokio::test(start_paused = true)]
async fn closes_idle_leases_and_can_reopen_without_leaking_transfer_capacity() {
    let f = fixture();
    let published = f.publish(b"immutable snapshot", "answer.md");
    let first = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    let second = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    tokio::time::advance(Duration::from_millis(5 * 60 * 1000 + 1)).await;
    assert_eq!(
        code(f.read(&first.transfer_id, 0, 1, &f.context)),
        ErrorCode::Expired
    );
    // The expired second lease no longer counts against this connection's two transfers either.
    f.service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    let again = f
        .service
        .open(&published.reference, Some(&f.context))
        .unwrap();
    assert_eq!(Some(again.attachment.id), published.reference.attachment_id);
    assert_eq!(
        code(f.read(&second.transfer_id, 0, 1, &f.context)),
        ErrorCode::Forbidden
    );
}

#[test]
fn uses_content_signatures_and_keeps_active_formats_download_only() {
    assert_eq!(
        attachment_content_type("fake.png", b"<script>alert(1)</script>"),
        "application/octet-stream"
    );
    assert_eq!(
        attachment_content_type("readme.md", b"# Notes"),
        "text/markdown"
    );
    assert_eq!(
        attachment_content_type("file.pdf", b"%PDF-1.7"),
        "application/pdf"
    );
    assert_eq!(
        attachment_content_type("drawing.svg", b"<svg/>"),
        "application/octet-stream"
    );
    assert_eq!(
        attachment_content_type("photo", &[137, 80, 78, 71, 13, 10, 26, 10, 0]),
        "image/png"
    );
    assert_eq!(
        attachment_content_type("song.m4a", b"\0\0\0\x20ftypM4A "),
        "audio/mp4"
    );
    assert_eq!(
        attachment_content_type("clip.mov", b"\0\0\0\x20ftypisom"),
        "video/mp4"
    );
    assert_eq!(
        attachment_content_type("notes.md", b"bin\0ary"),
        "application/octet-stream"
    );
}

#[test]
fn refuses_a_snapshot_outside_the_shared_files_staging_folder() {
    let f = fixture();
    let outside = f.root.join("private.md");
    fs::write(&outside, "private").unwrap();
    assert_eq!(
        code(f.service.register_snapshot(&outside, "private.md")),
        ErrorCode::BadParams
    );
    let inbox = Path::new(&f.bot.home_folder).join("inbox").join("note.md");
    fs::write(&inbox, "inbox").unwrap();
    assert_eq!(
        code(f.service.register_snapshot(&inbox, "note.md")),
        ErrorCode::BadParams,
        "only legacy adoption takes inbox files"
    );
}
