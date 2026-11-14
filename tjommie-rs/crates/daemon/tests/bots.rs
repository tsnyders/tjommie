//! `bots.test.ts`. Where the TypeScript test mocks `renameSync` or `rmSync` to fail, these cause the failure for real: an
//! open file blocks renaming its folder, and a deny-delete ACL stops a recursive removal partway. The TypeScript tests
//! also check the "Could not finish removing a deleted bot folder." log line; these check the behaviour around it.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{DenyDelete, TempRoot, insert_account, junction, make_default_account, test_db};
use daemon::bots::{
    StoredBot, bot_prompt, create_bot, delete_bot, get_stored_bot, list_stored_bots, set_paused,
    sweep_deleted_bot_folders, update_bot,
};
use daemon::ids::new_id;
use protocol::ErrorCode;
use protocol::model::{BotInput, BotPatch};
use rusqlite::Connection;
use serde_json::{Value, json};

struct Fixture {
    root: TempRoot,
    bots_dir: PathBuf,
    db: Connection,
}

fn fixture() -> Fixture {
    let root = TempRoot::new();
    let db = test_db(&root.0);
    Fixture {
        bots_dir: root.0.join("bots"),
        db,
        root,
    }
}

fn input(extra: Value) -> BotInput {
    let mut value = json!({ "name": "Researcher", "color": "teal", "shape": "droplet", "instructions": "Find sources.", "provider": "claude", "model": "sonnet" });
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}

fn patch(value: Value) -> BotPatch {
    serde_json::from_value(value).unwrap()
}

fn path(value: &Path) -> String {
    value.display().to_string()
}

fn count(db: &Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |row| row.get(0)).unwrap()
}

/// The error message, for the tests that match on it like `toThrow(/…/)`.
fn fails<T: std::fmt::Debug>(result: Result<T, daemon::server::RpcError>) -> String {
    result.unwrap_err().message
}

#[test]
fn pins_new_bots_to_the_provider_default_and_keeps_existing_pins_when_the_default_changes() {
    let f = fixture();
    insert_account(&f.db, "personal", "claude", &f.root.0.join("personal"));
    insert_account(&f.db, "work", "claude", &f.root.0.join("work"));
    let first = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    assert_eq!(first.account_id.as_deref(), Some("personal"));
    make_default_account(&f.db, "work");
    assert_eq!(
        get_stored_bot(&f.db, &first.id)
            .unwrap()
            .account_id
            .as_deref(),
        Some("personal")
    );
    assert_eq!(
        create_bot(&f.db, &f.bots_dir, &input(json!({ "name": "Second" })))
            .unwrap()
            .account_id
            .as_deref(),
        Some("work")
    );
}

#[test]
fn rejects_missing_and_cross_provider_account_pins_before_creating_a_home() {
    let f = fixture();
    insert_account(&f.db, "codex", "codex", &f.root.0.join("codex"));
    assert!(
        fails(create_bot(
            &f.db,
            &f.bots_dir,
            &input(json!({ "accountId": "missing" }))
        ))
        .contains("existing account")
    );
    assert!(
        fails(create_bot(
            &f.db,
            &f.bots_dir,
            &input(json!({ "accountId": "codex" }))
        ))
        .contains("provider")
    );
    assert_eq!(list_stored_bots(&f.db).unwrap(), []);
}

#[test]
fn clears_the_previous_accounts_limit_when_explicitly_switching_accounts() {
    let f = fixture();
    for id in ["personal", "work"] {
        insert_account(&f.db, id, "claude", &f.root.0.join(id));
    }
    let bot = create_bot(
        &f.db,
        &f.bots_dir,
        &input(json!({ "accountId": "personal" })),
    )
    .unwrap();
    f.db.execute(
        "UPDATE bots SET limited_until = '2099-01-01T00:00:00.000Z' WHERE id = ?1",
        [&bot.id],
    )
    .unwrap();
    let changed = update_bot(&f.db, &bot.id, &patch(json!({ "accountId": "work" }))).unwrap();
    assert_eq!(changed.account_id.as_deref(), Some("work"));
    assert_eq!(changed.limited_until, None);
    assert_eq!(changed.home_folder, bot.home_folder);
}

#[test]
fn creates_a_bot_with_defaults_a_home_folder_notes_md_and_an_inbox() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    assert_eq!(bot.name, "Researcher");
    assert_eq!(serde_json::to_value(bot.autonomy).unwrap(), "folder");
    assert!(!bot.browser && !bot.paused);
    assert_eq!(
        (
            bot.project_folder.as_deref(),
            bot.can_message.len(),
            bot.limited_until.as_deref()
        ),
        (None, 0, None)
    );
    assert_eq!(bot.home_folder, path(&f.bots_dir.join(&bot.id)));
    assert!(
        fs::read_to_string(Path::new(&bot.home_folder).join("notes.md"))
            .unwrap()
            .contains("Researcher")
    );
    assert!(Path::new(&bot.home_folder).join("inbox").is_dir());
}

#[test]
fn keeps_names_unique_regardless_of_case() {
    let f = fixture();
    create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    let error =
        create_bot(&f.db, &f.bots_dir, &input(json!({ "name": "researcher" }))).unwrap_err();
    assert_eq!(error.code, ErrorCode::NameTaken);
    assert!(error.message.contains("already an agent"));
}

#[test]
fn accepts_only_existing_absolute_project_folders_spaces_and_accents_included() {
    let f = fixture();
    let project = f.root.0.join("My Project ñ");
    fs::create_dir(&project).unwrap();
    let bot = create_bot(
        &f.db,
        &f.bots_dir,
        &input(json!({ "projectFolder": path(&project) })),
    )
    .unwrap();
    assert_eq!(bot.project_folder, Some(path(&project)));
    let missing = path(&f.root.0.join("missing"));
    assert!(
        fails(create_bot(
            &f.db,
            &f.bots_dir,
            &input(json!({ "name": "B", "projectFolder": missing }))
        ))
        .contains("existing folder")
    );
    assert!(
        fails(create_bot(
            &f.db,
            &f.bots_dir,
            &input(json!({ "name": "C", "projectFolder": "relative" }))
        ))
        .contains("existing folder")
    );
}

#[test]
fn validates_the_can_message_list() {
    let f = fixture();
    let researcher = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    let writer = create_bot(
        &f.db,
        &f.bots_dir,
        &input(json!({ "name": "Writer", "canMessage": [researcher.id] })),
    )
    .unwrap();
    assert_eq!(writer.can_message, std::slice::from_ref(&researcher.id));
    assert!(
        fails(update_bot(
            &f.db,
            &researcher.id,
            &patch(json!({ "canMessage": [researcher.id] }))
        ))
        .contains("itself")
    );
    assert!(
        fails(update_bot(
            &f.db,
            &researcher.id,
            &patch(json!({ "canMessage": ["nope"] }))
        ))
        .contains("no such agent")
    );
}

#[test]
fn changes_only_the_patched_fields_and_drops_saved_rules_when_the_provider_changes() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    f.db.execute(
        "INSERT INTO allow_rules (bot_id, rule) VALUES (?1, 'Write')",
        [&bot.id],
    )
    .unwrap();
    let changed = update_bot(&f.db, &bot.id, &patch(json!({ "autonomy": "ask" }))).unwrap();
    assert_eq!(serde_json::to_value(changed.autonomy).unwrap(), "ask");
    assert_eq!(
        (
            changed.name.as_str(),
            changed.model.as_str(),
            changed.instructions.as_str()
        ),
        ("Researcher", "sonnet", "Find sources.")
    );
    assert_eq!(count(&f.db, "SELECT COUNT(*) FROM allow_rules"), 1);
    update_bot(
        &f.db,
        &bot.id,
        &patch(json!({ "provider": "codex", "model": "gpt-5.6-sol" })),
    )
    .unwrap();
    assert_eq!(count(&f.db, "SELECT COUNT(*) FROM allow_rules"), 0);
}

#[test]
fn rejects_changing_provider_without_a_model() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    let error = update_bot(&f.db, &bot.id, &patch(json!({ "provider": "codex" }))).unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (
            ErrorCode::BadParams,
            "A model is required when changing provider"
        )
    );
}

#[test]
fn pauses_and_deletes_the_bot_with_its_chat_and_home_folder() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    set_paused(&f.db, &bot.id, true).unwrap();
    assert!(get_stored_bot(&f.db, &bot.id).unwrap().paused);
    f.db.execute(
        "INSERT INTO messages (id, conversation_id, author, kind, body, status, created_at, seq) VALUES ('m', ?1, 'you', 'text', 'hi', 'done', 'now', 1)",
        [&bot.id],
    )
    .unwrap();
    delete_bot(&f.db, &bot.id).unwrap();
    assert_eq!(list_stored_bots(&f.db).unwrap(), []);
    assert!(!Path::new(&bot.home_folder).exists());
    assert_eq!(count(&f.db, "SELECT COUNT(*) FROM messages"), 0);
}

#[test]
fn keeps_the_bot_when_its_home_folder_cannot_be_removed() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    let home = Path::new(&bot.home_folder);
    let notes = fs::read_to_string(home.join("notes.md")).unwrap();
    fs::write(home.join("inbox").join("keep.txt"), "Keep this file").unwrap();
    fs::create_dir(home.join("chrome-profile")).unwrap();
    fs::write(
        home.join("chrome-profile").join("keep.txt"),
        "Keep this profile",
    )
    .unwrap();
    // A program still holding a file in the folder, as a CLI or the browser can.
    let open = fs::File::open(home.join("chrome-profile").join("keep.txt")).unwrap();
    let error = delete_bot(&f.db, &bot.id).unwrap_err();
    drop(open);
    assert_eq!(
        (error.code, error.message.as_str()),
        (ErrorCode::Busy, "The agent folder is busy")
    );
    assert_eq!(get_stored_bot(&f.db, &bot.id).unwrap(), bot);
    assert_eq!(fs::read_to_string(home.join("notes.md")).unwrap(), notes);
    assert_eq!(
        fs::read_to_string(home.join("inbox").join("keep.txt")).unwrap(),
        "Keep this file"
    );
    assert_eq!(
        fs::read_to_string(home.join("chrome-profile").join("keep.txt")).unwrap(),
        "Keep this profile"
    );
}

#[test]
fn builds_a_prompt_with_the_name_home_folder_and_instructions() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    let prompt = bot_prompt(&bot);
    assert!(prompt.contains("You are Researcher"));
    assert!(prompt.contains(&bot.home_folder));
    assert!(prompt.contains("Find sources."));
}

#[test]
fn tells_the_bot_about_tjommies_tools_and_about_its_browser_only_when_it_has_one() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    let prompt = bot_prompt(&bot);
    for name in ["list_bots", "message_bot", "share_file", "add_routine"] {
        assert!(prompt.contains(name), "{name}");
    }
    assert!(!prompt.contains("browser tools"));
    assert!(
        bot_prompt(&StoredBot {
            browser: true,
            ..bot
        })
        .contains("browser tools")
    );
}

const MANAGED: &str = "The project folder must be outside the managed agents folder";
const DATA_ROOT: &str = "doesn't contain Tjommie's own data folder";

#[test]
fn rejects_creating_a_bot_with_a_project_at_or_below_the_managed_bots_folder() {
    let f = fixture();
    let owner = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    for project in [
        path(&f.bots_dir),
        owner.home_folder.clone(),
        path(&Path::new(&owner.home_folder).join("inbox")),
    ] {
        assert!(
            fails(create_bot(
                &f.db,
                &f.bots_dir,
                &input(json!({ "name": "Other", "projectFolder": project }))
            ))
            .contains(MANAGED)
        );
    }
    assert_eq!(list_stored_bots(&f.db).unwrap(), [owner]);
}

#[test]
fn rejects_updating_a_bot_with_a_project_at_or_below_the_managed_bots_folder_without_changing_it() {
    let f = fixture();
    let owner = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    let other = create_bot(&f.db, &f.bots_dir, &input(json!({ "name": "Other" }))).unwrap();
    let inbox = path(&Path::new(&owner.home_folder).join("inbox"));
    for project in [
        path(&f.bots_dir),
        owner.home_folder.clone(),
        inbox,
        other.home_folder.clone(),
    ] {
        let attempt = update_bot(
            &f.db,
            &other.id,
            &patch(json!({ "instructions": "Changed", "projectFolder": project })),
        );
        assert!(fails(attempt).contains(MANAGED));
        assert_eq!(get_stored_bot(&f.db, &other.id).unwrap(), other);
    }
}

#[test]
fn rejects_junction_aliases_into_the_managed_bots_folder_on_create_and_update() {
    let f = fixture();
    let owner = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    let alias = f.root.0.join("bots-alias");
    junction(&f.bots_dir, &alias);
    for project in [
        alias.clone(),
        alias.join(&owner.id),
        alias.join(&owner.id).join("inbox"),
    ] {
        assert!(
            fails(create_bot(
                &f.db,
                &f.bots_dir,
                &input(json!({ "name": "Other", "projectFolder": path(&project) }))
            ))
            .contains(MANAGED)
        );
        assert!(
            fails(update_bot(
                &f.db,
                &owner.id,
                &patch(json!({ "projectFolder": path(&project) }))
            ))
            .contains(MANAGED)
        );
    }
    assert_eq!(get_stored_bot(&f.db, &owner.id).unwrap(), owner);
}

#[test]
fn checks_the_real_managed_bots_folder_when_its_configured_path_is_a_junction() {
    let f = fixture();
    let physical = f.root.0.join("physical-bots");
    fs::create_dir(&physical).unwrap();
    junction(&physical, &f.bots_dir);
    let owner = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    assert!(
        fails(create_bot(
            &f.db,
            &f.bots_dir,
            &input(json!({ "name": "Other", "projectFolder": path(&physical) }))
        ))
        .contains(MANAGED)
    );
    let inside = path(&physical.join(&owner.id));
    assert!(
        fails(update_bot(
            &f.db,
            &owner.id,
            &patch(json!({ "projectFolder": inside }))
        ))
        .contains(MANAGED)
    );
}

#[test]
fn rejects_project_paths_under_the_managed_bots_folder_even_when_a_junction_leads_outside() {
    let f = fixture();
    let owner = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    let project = f.root.0.join("project");
    fs::create_dir(&project).unwrap();
    let alias = Path::new(&owner.home_folder).join("project-link");
    junction(&project, &alias);
    assert!(
        fails(create_bot(
            &f.db,
            &f.bots_dir,
            &input(json!({ "name": "Other", "projectFolder": path(&alias) }))
        ))
        .contains(MANAGED)
    );
    assert!(
        fails(update_bot(
            &f.db,
            &owner.id,
            &patch(json!({ "projectFolder": path(&alias) }))
        ))
        .contains(MANAGED)
    );
}

#[test]
fn accepts_a_separate_project_folder_whose_name_shares_the_managed_bots_prefix() {
    let f = fixture();
    let project = f.root.0.join("bots-project");
    fs::create_dir(&project).unwrap();
    let bot = create_bot(
        &f.db,
        &f.bots_dir,
        &input(json!({ "projectFolder": path(&project) })),
    )
    .unwrap();
    assert_eq!(bot.project_folder, Some(path(&project)));
    assert_eq!(
        update_bot(&f.db, &bot.id, &patch(json!({ "projectFolder": null })))
            .unwrap()
            .project_folder,
        None
    );
    assert_eq!(
        update_bot(
            &f.db,
            &bot.id,
            &patch(json!({ "projectFolder": path(&project) }))
        )
        .unwrap()
        .project_folder,
        Some(path(&project))
    );
}

#[test]
fn restores_the_untouched_folder_if_the_database_refuses_deletion() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    f.db.execute_batch("CREATE TRIGGER refuse_bot_delete BEFORE DELETE ON bots BEGIN SELECT RAISE(ABORT, 'keep bot'); END").unwrap();
    assert!(fails(delete_bot(&f.db, &bot.id)).contains("keep bot"));
    assert_eq!(get_stored_bot(&f.db, &bot.id).unwrap(), bot);
    assert!(
        fs::read_to_string(Path::new(&bot.home_folder).join("notes.md"))
            .unwrap()
            .contains("Researcher")
    );
}

fn deleting_leftovers(bots_dir: &Path, id: &str) -> Vec<PathBuf> {
    fs::read_dir(bots_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|entry| {
            entry
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("{id}.deleting-"))
        })
        .collect()
}

#[test]
fn does_not_restore_a_partially_removed_folder_after_successful_database_deletion() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    fs::write(
        Path::new(&bot.home_folder).join("inbox").join("locked.txt"),
        "locked child",
    )
    .unwrap();
    let denied = DenyDelete::new(Path::new(&bot.home_folder), "inbox/locked.txt");
    delete_bot(&f.db, &bot.id).unwrap();
    assert!(!Path::new(&bot.home_folder).exists());
    assert_eq!(list_stored_bots(&f.db).unwrap(), []);
    assert_eq!(deleting_leftovers(&f.bots_dir, &bot.id).len(), 1);
    drop(denied);
    sweep_deleted_bot_folders(&f.db, &f.bots_dir);
    assert_eq!(fs::read_dir(&f.bots_dir).unwrap().count(), 0);
}

#[test]
fn logs_leftover_cleanup_failures_and_retries_them_on_the_next_sweep() {
    let f = fixture();
    fs::create_dir(&f.bots_dir).unwrap();
    let folders = [
        f.bots_dir
            .join(format!("{}.deleting-{}", new_id(), new_id())),
        f.bots_dir
            .join(format!("{}.deleting-{}", new_id(), new_id())),
    ];
    for folder in &folders {
        fs::create_dir(folder).unwrap();
    }
    fs::write(folders[0].join("busy.txt"), "busy").unwrap();
    // Opened without FILE_SHARE_DELETE (Rust shares delete by default), so the file can't be removed while it's open.
    let open =
        std::os::windows::fs::OpenOptionsExt::share_mode(fs::OpenOptions::new().read(true), 0x1)
            .open(folders[0].join("busy.txt"))
            .unwrap();
    sweep_deleted_bot_folders(&f.db, &f.bots_dir);
    assert_eq!(folders.iter().filter(|folder| folder.exists()).count(), 1);
    drop(open);
    sweep_deleted_bot_folders(&f.db, &f.bots_dir);
    assert!(!folders.iter().any(|folder| folder.exists()));
}

#[test]
fn preserves_live_bot_folders_and_unrelated_entries_during_leftover_cleanup() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    let live_rename = PathBuf::from(format!("{}.deleting-{}", bot.home_folder, new_id()));
    fs::rename(&bot.home_folder, &live_rename).unwrap();
    let unrelated = f.bots_dir.join("notes.deleting-keep");
    fs::create_dir(&unrelated).unwrap();
    let ordinary = f.bots_dir.join("other-folder");
    fs::create_dir(&ordinary).unwrap();
    let file = f
        .bots_dir
        .join(format!("{}.deleting-{}", new_id(), new_id()));
    fs::write(&file, "Keep file").unwrap();
    sweep_deleted_bot_folders(&f.db, &f.bots_dir);
    assert_eq!(get_stored_bot(&f.db, &bot.id).unwrap(), bot);
    assert!(
        fs::read_to_string(live_rename.join("notes.md"))
            .unwrap()
            .contains("Researcher")
    );
    assert!(
        [unrelated, ordinary, file]
            .iter()
            .all(|entry| entry.exists())
    );
}

#[test]
fn keeps_a_registered_live_home_even_when_its_name_looks_like_another_bots_deletion_leftover() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    let home = f
        .bots_dir
        .join(format!("{}.deleting-{}", new_id(), new_id()));
    fs::rename(&bot.home_folder, &home).unwrap();
    f.db.execute(
        "UPDATE bots SET home_folder = ?1 WHERE id = ?2",
        [path(&home), bot.id.clone()],
    )
    .unwrap();
    sweep_deleted_bot_folders(&f.db, &f.bots_dir);
    assert!(
        fs::read_to_string(home.join("notes.md"))
            .unwrap()
            .contains("Researcher")
    );
}

#[test]
fn never_removes_junction_targets_while_sweeping_a_deleted_bots_folder() {
    let f = fixture();
    fs::create_dir(&f.bots_dir).unwrap();
    let outside = f.root.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep.txt"), "Do not remove").unwrap();
    let leftover = f
        .bots_dir
        .join(format!("{}.deleting-{}", new_id(), new_id()));
    fs::create_dir(&leftover).unwrap();
    junction(&outside, &leftover.join("link"));
    let linked_entry = f
        .bots_dir
        .join(format!("{}.deleting-{}", new_id(), new_id()));
    junction(&outside, &linked_entry);
    sweep_deleted_bot_folders(&f.db, &f.bots_dir);
    assert!(!leftover.exists());
    assert!(linked_entry.exists());
    assert_eq!(
        fs::read_to_string(outside.join("keep.txt")).unwrap(),
        "Do not remove"
    );
}

#[test]
fn rejects_the_data_root_and_its_ancestors_on_create_and_update() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    for project in [path(&f.root.0), path(f.root.0.parent().unwrap())] {
        assert!(
            fails(create_bot(
                &f.db,
                &f.bots_dir,
                &input(json!({ "name": "Other", "projectFolder": project }))
            ))
            .contains(DATA_ROOT)
        );
        assert!(
            fails(update_bot(
                &f.db,
                &bot.id,
                &patch(json!({ "projectFolder": project }))
            ))
            .contains(DATA_ROOT)
        );
    }
    assert_eq!(get_stored_bot(&f.db, &bot.id).unwrap(), bot);
}

#[test]
fn rejects_junction_aliases_of_the_data_root_and_its_ancestors() {
    let f = fixture();
    let bot = create_bot(&f.db, &f.bots_dir, &input(json!({}))).unwrap();
    let aliases = TempRoot::new();
    for (name, target) in [
        ("root", f.root.0.clone()),
        ("parent", f.root.0.parent().unwrap().to_path_buf()),
    ] {
        let alias = aliases.0.join(name);
        junction(&target, &alias);
        assert!(
            fails(create_bot(
                &f.db,
                &f.bots_dir,
                &input(json!({ "name": "Other", "projectFolder": path(&alias) }))
            ))
            .contains(DATA_ROOT)
        );
        assert!(
            fails(update_bot(
                &f.db,
                &bot.id,
                &patch(json!({ "projectFolder": path(&alias) }))
            ))
            .contains(DATA_ROOT)
        );
    }
}

#[test]
fn checks_the_physical_data_root_when_its_configured_path_is_a_junction() {
    let f = fixture();
    let physical_parent = TempRoot::new();
    let physical_root = physical_parent.0.join("data");
    fs::create_dir(&physical_root).unwrap();
    let alias = f.root.0.join("data-link");
    junction(&physical_root, &alias);
    let aliased_bots = alias.join("bots");
    let bot = create_bot(&f.db, &aliased_bots, &input(json!({}))).unwrap();
    for project in [
        physical_root.clone(),
        physical_parent.0.clone(),
        alias.clone(),
        f.root.0.clone(),
    ] {
        assert!(
            fails(create_bot(
                &f.db,
                &aliased_bots,
                &input(json!({ "name": "Other", "projectFolder": path(&project) }))
            ))
            .contains(DATA_ROOT)
        );
        assert!(
            fails(update_bot(
                &f.db,
                &bot.id,
                &patch(json!({ "projectFolder": path(&project) }))
            ))
            .contains(DATA_ROOT)
        );
    }
}
