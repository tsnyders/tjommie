//! Port of `worktrees.test.ts`, one test per `it(...)`. The TypeScript fixture (`project-test-support.ts`) is rebuilt
//! here with the same files, commits and Git environment: an owner repository on `main`, a managed root, and an empty
//! global config. The database, bots and projects that the TypeScript fixture also builds are not used by these
//! tests, so they are not rebuilt. Only temp folders are used.
#![cfg(windows)]

use daemon::fsid::is_within;
use daemon::ids::{new_id, now_iso};
use daemon::project_review::review_snapshot;
use daemon::spawn;
use daemon::worktrees::{ManagedWorktree, Repository, Worktrees};
use sha2::{Digest, Sha256};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// The environment `projectFixture` gives its Git calls: an empty global config and a fixed author and committer.
fn git_environment(config: &Path) -> Vec<(&'static str, OsString)> {
    let mut environment = vec![
        ("GIT_CONFIG_NOSYSTEM", OsString::from("1")),
        ("GIT_CONFIG_GLOBAL", config.as_os_str().to_owned()),
        ("GIT_AUTHOR_NAME", OsString::from("Synthetic fixture")),
        (
            "GIT_AUTHOR_EMAIL",
            OsString::from("fixture@example.invalid"),
        ),
        ("GIT_COMMITTER_NAME", OsString::from("Synthetic fixture")),
        (
            "GIT_COMMITTER_EMAIL",
            OsString::from("fixture@example.invalid"),
        ),
    ];
    for key in [
        "PATH",
        "SystemRoot",
        "SystemDrive",
        "WINDIR",
        "PATHEXT",
        "TEMP",
        "TMP",
        "COMSPEC",
    ] {
        if let Some(value) = env::var_os(key) {
            environment.push((key, value));
        }
    }
    environment
}

/// `projectFixture`'s `git(path, ...args)`: real Git, trimmed stdout, and a path that must stay inside the fixture.
fn git_in(root: &Path, config: &Path, path: &Path, args: &[&str]) -> String {
    assert!(
        is_within(path, root).expect("the path can be inspected"),
        "{} is outside the fixture",
        path.display()
    );
    let output = spawn::command("git")
        .args(args)
        .current_dir(path)
        .env_clear()
        .envs(git_environment(config))
        .output()
        .expect("git starts");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// The trimmed UTF-8 stdout of a `Worktrees::git` call, as the TypeScript tests read it.
fn stdout(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    config: PathBuf,
    repo_path: PathBuf,
    base: String,
    worktrees: Worktrees,
    repository: Repository,
}

/// `h.owner()`: the owner checkout's HEAD, status and index, which no managed-worktree call may change.
#[derive(Debug, PartialEq)]
struct Owner {
    head: String,
    status: String,
    index: Vec<u8>,
}

impl Fixture {
    fn git(&self, path: &Path, args: &[&str]) -> String {
        git_in(&self.root, &self.config, path, args)
    }

    fn owner(&self) -> Owner {
        Owner {
            head: self.git(&self.repo_path, &["rev-parse", "HEAD"]),
            status: self.git(&self.repo_path, &["status", "--porcelain=v1"]),
            index: fs::read(self.repo_path.join(".git/index"))
                .expect("the owner index is readable"),
        }
    }
}

/// `projectFixture()`: the owner repository on `main` with `src/a.txt`, `src/b.txt`, a `.gitignore` and the check
/// script, one `Synthetic base` commit, and a `Worktrees` over `managed`.
fn fixture() -> Fixture {
    let dir = TempDir::new().expect("a temp folder");
    let root = dir.path().to_path_buf();
    let config = root.join("empty-gitconfig");
    fs::write(&config, "").expect("the empty global config is written");
    let repo_path = root.join("owner");
    fs::create_dir(&repo_path).expect("the owner folder is made");
    let git = |path: &Path, args: &[&str]| git_in(&root, &config, path, args);
    git(&repo_path, &["init", "-b", "main"]);
    git(&repo_path, &["config", "user.name", "Synthetic fixture"]);
    git(
        &repo_path,
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(&repo_path, &["config", "commit.gpgsign", "false"]);
    fs::create_dir(repo_path.join("src")).expect("the src folder is made");
    fs::write(repo_path.join("src/a.txt"), "A0\n").expect("a.txt is written");
    fs::write(repo_path.join("src/b.txt"), "B0\n").expect("b.txt is written");
    fs::write(repo_path.join(".gitignore"), "check-output.txt\n").expect(".gitignore is written");
    fs::write(
        repo_path.join("check.mjs"),
        r#"import{writeFileSync}from"node:fs";writeFileSync("check-output.txt","passed");"#,
    )
    .expect("check.mjs is written");
    git(&repo_path, &["add", "."]);
    git(&repo_path, &["commit", "-m", "Synthetic base"]);
    let base = git(&repo_path, &["rev-parse", "HEAD"]);
    let worktrees = Worktrees::new(&root.join("managed")).expect("the managed root is made");
    let info = worktrees
        .repository(repo_path.to_str().expect("a UTF-8 temp path"))
        .expect("the owner folder is a repository");
    let repository = Repository {
        id: new_id(),
        path: info.path,
        common_dir: info.common_dir,
        name: info.name,
        created_at: now_iso(),
    };
    Fixture {
        _dir: dir,
        root,
        config,
        repo_path,
        base,
        worktrees,
        repository,
    }
}

#[test]
fn materializes_independent_exact_git_objects_without_running_repository_checkout_hooks_or_filters()
{
    let h = fixture();
    fs::write(
        h.repo_path.join(".git/hooks/post-checkout"),
        "#!/bin/sh\ntouch ../hook-ran\n",
    )
    .unwrap();
    h.git(
        &h.repo_path,
        &["config", "filter.untrusted.smudge", "touch ../filter-ran"],
    );
    fs::write(
        h.repo_path.join(".gitattributes"),
        "src/* filter=untrusted\n",
    )
    .unwrap();
    h.git(&h.repo_path, &["add", ".gitattributes"]);
    h.git(&h.repo_path, &["commit", "-m", "Synthetic attributes"]);
    let sha = h.git(&h.repo_path, &["rev-parse", "HEAD"]);
    let a = h.worktrees.describe("a", &sha, None).unwrap();
    let b = h.worktrees.describe("b", &sha, None).unwrap();
    let before = h.owner();
    h.worktrees.create(&h.repository, &a, false).unwrap();
    h.worktrees.create(&h.repository, &b, false).unwrap();
    assert_eq!(
        fs::read_to_string(Path::new(&a.path).join("src/a.txt")).unwrap(),
        "A0\n"
    );
    assert!(!h.root.join("hook-ran").exists());
    assert!(!h.root.join("filter-ran").exists());
    assert!(!h.worktrees.dirty(&h.repository, &a, None, false).unwrap());
    fs::write(Path::new(&a.path).join("src/a.txt"), "working A\n").unwrap();
    assert!(h.worktrees.dirty(&h.repository, &a, None, false).unwrap());
    assert_eq!(
        fs::read_to_string(Path::new(&b.path).join("src/a.txt")).unwrap(),
        "A0\n"
    );
    assert_eq!(h.owner(), before);
}

#[test]
fn merges_an_exact_accepted_commit_in_an_owned_candidate_and_blocks_a_users_checked_out_target() {
    let h = fixture();
    let task = h.worktrees.describe("task", &h.base, None).unwrap();
    h.worktrees.create(&h.repository, &task, false).unwrap();
    let task_path = PathBuf::from(&task.path);
    fs::write(task_path.join("src/a.txt"), "A1\n").unwrap();
    h.git(&task_path, &["add", "src/a.txt"]);
    h.git(&task_path, &["commit", "-m", "Accepted task"]);
    let accepted = h.worktrees.head(&h.repository, &task).unwrap();
    fs::write(h.repo_path.join("src/b.txt"), "owner staged\n").unwrap();
    h.git(&h.repo_path, &["add", "src/b.txt"]);
    fs::write(h.repo_path.join("src/b.txt"), "owner unstaged\n").unwrap();
    fs::write(h.repo_path.join("owner-note"), "owner untracked\n").unwrap();
    let before = h.owner();
    let bytes = format!(
        "{:x}",
        Sha256::digest(fs::read(h.repo_path.join("src/b.txt")).unwrap())
    );
    let candidate = h.worktrees.describe("candidate", &h.base, None).unwrap();
    let merged = h
        .worktrees
        .merge_candidate(&h.repository, &candidate, &h.base, &accepted)
        .unwrap();
    assert!(!merged.conflict);
    let sha = merged.sha.expect("a clean merge has a commit");
    assert!(
        !h.worktrees
            .dirty(&h.repository, &candidate, None, false)
            .unwrap()
    );
    assert_eq!(
        h.worktrees
            .promote(&h.repository, "refs/heads/main", &h.base, &sha)
            .unwrap()
            .reason
            .as_deref(),
        Some("target_checked_out")
    );
    h.git(&h.repo_path, &["branch", "delivery", h.base.as_str()]);
    assert_eq!(
        h.worktrees
            .promote(&h.repository, "refs/heads/delivery", &h.base, &sha)
            .unwrap()
            .reason,
        None
    );
    assert_eq!(h.git(&h.repo_path, &["rev-parse", "delivery"]), sha);
    assert_eq!(h.owner(), before);
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(fs::read(h.repo_path.join("src/b.txt")).unwrap())
        ),
        bytes
    );
}

#[test]
fn preserves_conflict_marker_source_and_refuses_cleanup_of_dirty_or_foreign_worktrees() {
    let h = fixture();
    let a = h.worktrees.describe("left", &h.base, None).unwrap();
    let b = h.worktrees.describe("right", &h.base, None).unwrap();
    for (work, text) in [(&a, "LEFT\n"), (&b, "RIGHT\n")] {
        h.worktrees.create(&h.repository, work, false).unwrap();
        let path = PathBuf::from(&work.path);
        fs::write(path.join("src/a.txt"), text).unwrap();
        h.git(&path, &["add", "src/a.txt"]);
        h.git(&path, &["commit", "-m", "Synthetic conflict"]);
    }
    let target = h.worktrees.head(&h.repository, &a).unwrap();
    let accepted = h.worktrees.head(&h.repository, &b).unwrap();
    let work = h.worktrees.describe("conflicted", &target, None).unwrap();
    assert!(
        h.worktrees
            .merge_candidate(&h.repository, &work, &target, &accepted)
            .unwrap()
            .conflict
    );
    assert!(
        fs::read_to_string(Path::new(&work.path).join("src/a.txt"))
            .unwrap()
            .contains("<<<<<<<")
    );
    let error = h
        .worktrees
        .remove(&h.repository, &work, &target)
        .unwrap_err();
    assert!(error.message.contains("Preserve"), "{}", error.message);
    let foreign = ManagedWorktree {
        path: h.repo_path.to_string_lossy().into_owned(),
        ..work.clone()
    };
    assert!(h.worktrees.assert_owned(&h.repository, &foreign).is_err());
    assert_eq!(h.git(&h.repo_path, &["rev-parse", "HEAD"]), h.base);
}

#[test]
fn offers_immutable_source_only_review_and_fails_on_incomplete_binary_coverage() {
    let h = fixture();
    let snapshot = review_snapshot(&h.worktrees, &h.repository, &h.base, &h.base).unwrap();
    assert_eq!(snapshot.read("src/a.txt").unwrap(), "A0\n");
    // `Object.isFrozen(snapshot)` has no Rust counterpart: the snapshot offers only `read`, so nothing can change it.
    assert!(
        snapshot
            .read("../owner-note")
            .unwrap_err()
            .contains("outside")
    );
    fs::write(h.repo_path.join("src/a.txt"), "new uncommitted bytes\n").unwrap();
    assert_eq!(snapshot.read("src/a.txt").unwrap(), "A0\n");
    fs::write(h.repo_path.join("binary.bin"), [0u8, 1, 2]).unwrap();
    h.git(&h.repo_path, &["add", "binary.bin"]);
    h.git(&h.repo_path, &["commit", "-m", "Synthetic binary"]);
    let head = h.git(&h.repo_path, &["rev-parse", "HEAD"]);
    let Err(error) = review_snapshot(&h.worktrees, &h.repository, &h.base, &head) else {
        panic!("binary coverage should be incomplete");
    };
    assert!(error.contains("binary"), "{error}");
}

#[test]
fn rejects_hostile_git_tree_names_before_creating_a_worktree() {
    let h = fixture();
    let oid = stdout(
        h.worktrees
            .git(
                &h.repo_path,
                &["hash-object", "-w", "--stdin"],
                Some("Synthetic object\n".as_bytes()),
            )
            .unwrap(),
    );
    for (index, name) in [".GIT", "NUL.txt"].into_iter().enumerate() {
        let listing = format!("100644 blob {oid}\t{name}\n");
        let tree = stdout(
            h.worktrees
                .git(&h.repo_path, &["mktree"], Some(listing.as_bytes()))
                .unwrap(),
        );
        let commit = stdout(
            h.worktrees
                .git(
                    &h.repo_path,
                    &["commit-tree", tree.as_str(), "-p", h.base.as_str()],
                    Some("Synthetic hostile tree\n".as_bytes()),
                )
                .unwrap(),
        );
        let work = h
            .worktrees
            .describe(&format!("unsafe-{index}"), &commit, None)
            .unwrap();
        let error = h.worktrees.create(&h.repository, &work, false).unwrap_err();
        assert!(
            error.message.contains("unsupported_tree_path"),
            "{}",
            error.message
        );
        assert!(!Path::new(&work.path).exists());
    }
}

#[test]
fn materializes_a_directory_to_file_change_without_retaining_deleted_tracked_files() {
    let h = fixture();
    let task = h.worktrees.describe("refactor", &h.base, None).unwrap();
    h.worktrees.create(&h.repository, &task, false).unwrap();
    let task_path = PathBuf::from(&task.path);
    fs::remove_file(task_path.join("src/a.txt")).unwrap();
    fs::remove_file(task_path.join("src/b.txt")).unwrap();
    fs::remove_dir(task_path.join("src")).unwrap();
    fs::write(task_path.join("src"), "replacement file\n").unwrap();
    h.git(&task_path, &["add", "-A"]);
    h.git(
        &task_path,
        &["commit", "-m", "Synthetic directory refactor"],
    );
    let candidate = h
        .worktrees
        .describe("refactor-candidate", &h.base, None)
        .unwrap();
    let head = h.worktrees.head(&h.repository, &task).unwrap();
    let merged = h
        .worktrees
        .merge_candidate(&h.repository, &candidate, &h.base, &head)
        .unwrap();
    assert!(!merged.conflict);
    let candidate_path = PathBuf::from(&candidate.path);
    assert_eq!(
        fs::read_to_string(candidate_path.join("src")).unwrap(),
        "replacement file\n"
    );
    assert!(
        !h.worktrees
            .dirty(&h.repository, &candidate, None, false)
            .unwrap()
    );
    assert!(h.repo_path.join("src/a.txt").exists());
}

#[test]
fn uses_actual_git_object_bytes_even_when_the_repository_has_replacement_refs() {
    let h = fixture();
    let original = h.git(&h.repo_path, &["rev-parse", "HEAD:src/a.txt"]);
    let replacement = stdout(
        h.worktrees
            .git(
                &h.repo_path,
                &["hash-object", "-w", "--stdin"],
                Some("REPLACED\n".as_bytes()),
            )
            .unwrap(),
    );
    h.git(
        &h.repo_path,
        &["replace", original.as_str(), replacement.as_str()],
    );
    assert_eq!(h.git(&h.repo_path, &["show", "HEAD:src/a.txt"]), "REPLACED");
    let work = h
        .worktrees
        .describe("actual-objects", &h.base, None)
        .unwrap();
    h.worktrees.create(&h.repository, &work, false).unwrap();
    assert_eq!(
        fs::read_to_string(Path::new(&work.path).join("src/a.txt")).unwrap(),
        "A0\n"
    );
    assert!(
        !h.worktrees
            .dirty(&h.repository, &work, None, false)
            .unwrap()
    );
}

#[test]
fn preserves_ignored_target_files_when_a_later_accepted_commit_starts_tracking_their_path() {
    let h = fixture();
    h.git(&h.repo_path, &["branch", "delivery", h.base.as_str()]);
    let first = h
        .worktrees
        .promote(&h.repository, "refs/heads/delivery", &h.base, &h.base)
        .unwrap();
    assert!(first.reason.is_none());
    let first_path = first.path.expect("the target worktree is reported");
    let retained = PathBuf::from(&first_path).join("check-output.txt");
    fs::write(&retained, "owner bytes\n").unwrap();
    let task = h
        .worktrees
        .describe("track-ignored", &h.base, None)
        .unwrap();
    h.worktrees.create(&h.repository, &task, false).unwrap();
    let task_path = PathBuf::from(&task.path);
    fs::write(task_path.join("check-output.txt"), "candidate bytes\n").unwrap();
    h.git(&task_path, &["add", "-f", "check-output.txt"]);
    h.git(&task_path, &["commit", "-m", "Synthetic tracked output"]);
    let candidate = h
        .worktrees
        .describe("track-ignored-candidate", &h.base, None)
        .unwrap();
    let head = h.worktrees.head(&h.repository, &task).unwrap();
    let merged = h
        .worktrees
        .merge_candidate(&h.repository, &candidate, &h.base, &head)
        .unwrap();
    let sha = merged.sha.expect("a clean merge has a commit");
    assert_eq!(
        h.worktrees
            .promote(&h.repository, "refs/heads/delivery", &h.base, &sha)
            .unwrap()
            .reason
            .as_deref(),
        Some("promotion_worktree_dirty")
    );
    assert_eq!(fs::read_to_string(&retained).unwrap(), "owner bytes\n");
    assert_eq!(h.git(&h.repo_path, &["rev-parse", "delivery"]), h.base);
}
