//! Port of `worktrees.ts`: the managed Git worktrees that project tasks run in, their owner markers, and the checks
//! that keep repository hooks, filters, fsmonitor and replacement refs out of every Git call. Windows semantics
//! throughout (the TypeScript `win32` branches); "same folder" and "inside folder" decisions go through `fsid`.

use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::thread;

use protocol::ErrorCode;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::fsid::{is_within, real_path, same_location};
use crate::server::RpcError;
use crate::spawn;

const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_TREE: u64 = 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
/// `Math.max(MAX_FILE + 65_536, 4 MiB)`: the stdout limit of `execFileSync` in `git()`.
const MAX_OUTPUT: u64 = MAX_FILE + 65_536;
/// The `spawnSync` maxBuffer of `mergeCandidate`.
const MERGE_OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;
/// `spawnSync`'s default `maxBuffer`, for the calls that only need an exit status.
const STATUS_OUTPUT_LIMIT: u64 = 1024 * 1024;
const BATCH_BYTES: u64 = 16 * 1024 * 1024;
const DIFF_LIMIT: usize = 512 * 1024;
const REPARSE_POINT: u32 = 0x400;
const ENV_KEYS: [&str; 11] = [
    "PATH",
    "SystemRoot",
    "SystemDrive",
    "WINDIR",
    "PATHEXT",
    "TEMP",
    "TMP",
    "COMSPEC",
    "HOME",
    "USERPROFILE",
    "XDG_CONFIG_HOME",
];
const GIT_FAILED: &str =
    "Git could not complete this operation. The repository and managed worktree were retained.";
const GRAFTS: &str = "Legacy Git grafts require reconciliation before exact-commit task work.";
const UNSUPPORTED_ENTRY: &str =
    "unsupported_tree_entry: symlinks and submodules require a supported isolated checkout.";
const UNSUPPORTED_PATH: &str = "unsupported_tree_path";
const OBJECT_CHANGED: &str = "Git object changed during checkout.";
const MERGE_UNAVAILABLE: &str = "merge_unavailable";

static BRANCH_REF: LazyLock<Regex> =
    LazyLock::new(|| fixed(r"^refs/heads/[A-Za-z0-9][A-Za-z0-9/._-]*$"));
static COMMIT_SHA: LazyLock<Regex> =
    LazyLock::new(|| fixed(r"^(?:[a-f0-9]{40}|[a-f0-9]{64})$"));
static WORKTREE_ID: LazyLock<Regex> = LazyLock::new(|| fixed(r"^[A-Za-z0-9_-]{1,100}$"));
static INVALID_NAME_CHARS: LazyLock<Regex> =
    LazyLock::new(|| fixed(r#"[<>:"|?*\x00-\x1f]"#));
static TRAILING_DOT_OR_SPACE: LazyLock<Regex> = LazyLock::new(|| fixed(r"[. ]$"));
/// The 8.3 alias `GIT~1` of `.git`, which a volume with short names makes point at the same file (a hardening over the
/// TypeScript, which lacks it).
static GIT_SHORT_NAME: LazyLock<Regex> = LazyLock::new(|| fixed(r"(?i)^git~\d+$"));
static RESERVED_NAME: LazyLock<Regex> =
    LazyLock::new(|| fixed(r"(?i)^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\..*)?$"));

/// A fixed pattern from this file; it always compiles.
fn fixed(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a fixed pattern compiles")
}

/// `Repository` (`worktrees.ts:8`): a registered repository, as the project store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub id: String,
    pub path: String,
    pub common_dir: String,
    pub name: String,
    pub created_at: String,
}

/// `Omit<Repository, "id" | "createdAt">` (`worktrees.ts:50`): what `Worktrees::repository` finds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryInfo {
    pub path: String,
    pub common_dir: String,
    pub name: String,
}

/// `ManagedWorktree` (`worktrees.ts:9`): one worktree this daemon owns, under the managed root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWorktree {
    pub id: String,
    pub path: String,
    pub branch: String,
    pub base_sha: String,
}

/// The `{ text, truncated }` that `diff` returns (`worktrees.ts:186`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffText {
    pub text: String,
    pub truncated: bool,
}

/// The `{ sha, conflict }` that `mergeCandidate` returns (`worktrees.ts:206`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeCandidate {
    pub sha: Option<String>,
    pub conflict: bool,
}

/// The `{ path, reason }` that `promote` returns (`worktrees.ts:223`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    pub path: Option<String>,
    pub reason: Option<String>,
}

/// `CommitSha.parse` (`protocol/src/projects.ts:4`): a full lowercase SHA-1 or SHA-256 commit id, else `bad_params`.
pub fn commit_sha(value: &str) -> Result<String, RpcError> {
    if COMMIT_SHA.is_match(value) {
        Ok(value.to_owned())
    } else {
        Err(RpcError::new(ErrorCode::BadParams, "Invalid commit SHA."))
    }
}

/// `Worktrees` (`worktrees.ts:17`): the managed worktree root, its empty hook folder, and the Git calls that run
/// against repositories and worktrees under it. Every method blocks on git, so callers use the blocking pool.
pub struct Worktrees {
    root: PathBuf,
    hooks: PathBuf,
}

/// What one git run returned. `code` is `None` when the process ended without an exit code.
struct GitOutput {
    code: Option<i32>,
    stdout: Vec<u8>,
    /// Stdout or stderr went over the limit. The excess was drained and dropped, like `execFileSync`'s `maxBuffer`
    /// failing the call.
    exceeded: bool,
}

/// The owner marker `create` writes: `{ ...work, commonDir }`, keys in `JSON.stringify` order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnerMarker<'a> {
    id: &'a str,
    path: &'a str,
    branch: &'a str,
    base_sha: &'a str,
    common_dir: &'a str,
}

/// One tree entry from `#entries` (`worktrees.ts:82`).
struct TreeEntry {
    path: String,
    oid: String,
    /// Kept for parity with the TypeScript entry; Windows has no Unix mode bits to set.
    #[allow(dead_code)]
    mode: String,
    size: u64,
}

impl Worktrees {
    /// `new Worktrees(root)` (`worktrees.ts:20`): creates the managed root with its empty hook folder and its owner
    /// markers folder, and keeps the canonical root.
    pub fn new(root: &Path) -> Result<Worktrees, RpcError> {
        fs::create_dir_all(root)?;
        let root = real_path(root)?;
        let hooks = root.join(".empty-hooks");
        fs::create_dir_all(&hooks)?;
        fs::create_dir_all(root.join(".owners"))?;
        Ok(Worktrees { root, hooks })
    }

    /// `root` (`worktrees.ts:18`): the canonical managed-worktree root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `git(path, args, input)` (`worktrees.ts:41`): runs git in `path` with hooks, fsmonitor and replace refs off and
    /// the scrubbed environment. A spawn failure, a non-zero exit or stdout over the limit is the same `bad_state`.
    pub fn git(
        &self,
        path: &Path,
        args: &[&str],
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>, RpcError> {
        match self.run(path, args, input, MAX_OUTPUT) {
            Ok(output) if output.code == Some(0) && !output.exceeded => Ok(output.stdout),
            _ => Err(reason(GIT_FAILED)),
        }
    }

    /// `#text` (`worktrees.ts:48`): the trimmed UTF-8 stdout of a git call.
    fn text(&self, path: &Path, args: &[&str]) -> Result<String, RpcError> {
        Ok(String::from_utf8_lossy(&self.git(path, args, None)?)
            .trim()
            .to_owned())
    }

    /// `repository(path)` (`worktrees.ts:50`): the canonical root of a Git repository, its common directory and its
    /// name. Only the repository's own root folder qualifies, and legacy grafts are refused.
    pub fn repository(&self, path: &str) -> Result<RepositoryInfo, RpcError> {
        let canonical = real_path(Path::new(path)).map_err(|_| {
            RpcError::new(
                ErrorCode::BadParams,
                "Choose an existing Git repository on this host.",
            )
        })?;
        let top = self.text(&canonical, &["rev-parse", "--show-toplevel"])?;
        let top = real_path(Path::new(&top))?;
        if !same_location(&top, &canonical)? {
            return Err(RpcError::new(
                ErrorCode::BadParams,
                "Choose the repository's root folder.",
            ));
        }
        let common = self.text(&canonical, &["rev-parse", "--git-common-dir"])?;
        let common_dir = real_path(&canonical.join(common))?;
        if common_dir.join("info").join("grafts").exists() {
            return Err(reason(GRAFTS));
        }
        Ok(RepositoryInfo {
            path: path_text(&canonical),
            common_dir: path_text(&common_dir),
            name: canonical
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        })
    }

    /// `assertRepository` (`worktrees.ts:60`): the repository is still the one that was registered.
    pub fn assert_repository(&self, repo: &Repository) -> Result<(), RpcError> {
        let common = Path::new(&repo.common_dir);
        if common.join("info").join("grafts").exists() {
            return Err(reason(GRAFTS));
        }
        let path = Path::new(&repo.path);
        // By spelling, not identity (`equalPath` in TypeScript): identity follows links, so a junction put in place of
        // the registered folder would compare equal to its target.
        if !same_spelling(&real_path(path)?, path) {
            return Err(reason("repository_changed"));
        }
        let real_common =
            real_path(&path.join(self.text(path, &["rev-parse", "--git-common-dir"])?))?;
        if !same_spelling(&real_common, common) {
            return Err(reason("repository_changed"));
        }
        Ok(())
    }

    /// `validateRef` (`worktrees.ts:66`): a full local branch ref that Git itself accepts.
    pub fn validate_ref(&self, repo: &Repository, r#ref: &str) -> Result<(), RpcError> {
        if !BRANCH_REF.is_match(r#ref) {
            return Err(RpcError::new(
                ErrorCode::BadParams,
                "Choose a full local branch ref.",
            ));
        }
        self.git(Path::new(&repo.path), &["check-ref-format", r#ref], None)?;
        Ok(())
    }

    /// `resolveRef` (`worktrees.ts:71`): the commit a branch ref points at, checked against the repository first.
    pub fn resolve_ref(&self, repo: &Repository, r#ref: &str) -> Result<String, RpcError> {
        self.assert_repository(repo)?;
        self.validate_ref(repo, r#ref)?;
        let target = format!("{}^{{commit}}", r#ref);
        commit_sha(&self.text(
            Path::new(&repo.path),
            &["rev-parse", "--verify", target.as_str()],
        )?)
    }

    /// `describe(id, baseSha, branch)` (`worktrees.ts:76`): the managed worktree for `id`, on `branch` (default
    /// `refs/heads/tjommie/<id>`).
    pub fn describe(
        &self,
        id: &str,
        base_sha: &str,
        branch: Option<&str>,
    ) -> Result<ManagedWorktree, RpcError> {
        if !WORKTREE_ID.is_match(id) || RESERVED_NAME.is_match(id) {
            return Err(RpcError::new(
                ErrorCode::BadParams,
                "Invalid managed worktree identity.",
            ));
        }
        commit_sha(base_sha)?;
        Ok(ManagedWorktree {
            id: id.to_owned(),
            path: path_text(&self.root.join(id)),
            branch: branch.map_or_else(|| format!("refs/heads/tjommie/{id}"), str::to_owned),
            base_sha: base_sha.to_owned(),
        })
    }

    /// `#entries` (`worktrees.ts:82`): the regular-file entries of a commit's tree, refusing anything a checkout on
    /// Windows could misplace, and the size limits.
    fn entries(&self, repo: &Repository, sha: &str) -> Result<Vec<TreeEntry>, RpcError> {
        commit_sha(sha)?;
        let listing = self.git(
            Path::new(&repo.path),
            &["ls-tree", "-r", "-l", "-z", sha],
            None,
        )?;
        let mut entries: Vec<TreeEntry> = Vec::new();
        let mut paths = HashSet::new();
        let mut total: u64 = 0;
        for record in String::from_utf8_lossy(&listing)
            .split('\0')
            .filter(|record| !record.is_empty())
        {
            let tab = record
                .find('\t')
                .ok_or_else(|| reason(UNSUPPORTED_ENTRY))?;
            let mut fields = record[..tab]
                .trim()
                .split(' ')
                .filter(|field| !field.is_empty());
            let (mode, kind, oid, raw_size) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            );
            let path = &record[tab + 1..];
            let key = path.to_lowercase();
            if kind != Some("blob") || !matches!(mode, Some("100644" | "100755")) {
                return Err(reason(UNSUPPORTED_ENTRY));
            }
            if path.is_empty()
                || path.contains('\\')
                || path.contains(char::REPLACEMENT_CHARACTER)
                || path.split('/').any(unsafe_name)
                || paths.contains(&key)
            {
                return Err(reason(UNSUPPORTED_PATH));
            }
            paths.insert(key);
            let size = raw_size.and_then(|value| value.parse::<u64>().ok());
            total = total.saturating_add(size.unwrap_or(0));
            let Some(size) = size.filter(|size| {
                *size <= MAX_FILE && total <= MAX_TREE && entries.len() < MAX_ENTRIES
            }) else {
                return Err(reason("worktree_size_limit"));
            };
            entries.push(TreeEntry {
                path: path.to_owned(),
                oid: oid.unwrap_or_default().to_owned(),
                mode: mode.unwrap_or_default().to_owned(),
                size,
            });
        }
        Ok(entries)
    }

    /// `#materialize` (`worktrees.ts:104`): makes the worktree's files exactly the tree at `sha`, reading the bytes
    /// from the repository's objects (never from its checkout hooks or filters).
    fn materialize(
        &self,
        repo: &Repository,
        work: &ManagedWorktree,
        sha: &str,
        entries: &[TreeEntry],
    ) -> Result<(), RpcError> {
        let work_path = Path::new(&work.path);
        let next: HashSet<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
        for old in nul_separated(&self.git(work_path, &["ls-files", "-z"], None)?) {
            if next.contains(old.as_str()) {
                continue;
            }
            let path = safe_path(work_path, &old)?;
            if path.exists() {
                fs::remove_file(&path)?;
            }
            // A lexical walk up to the worktree root: an identity match here would stop early at a link.
            let mut parent = path.parent();
            while let Some(folder) = parent.filter(|folder| !same_spelling(folder, work_path)) {
                match fs::remove_dir(folder) {
                    Ok(()) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::AlreadyExists
                        ) =>
                    {
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                parent = folder.parent();
            }
        }
        self.git(work_path, &["read-tree", sha], None)?;
        let mut start = 0;
        while start < entries.len() {
            let mut end = start + 1;
            let mut bytes = entries[start].size;
            while end < entries.len() && bytes + entries[end].size < BATCH_BYTES {
                bytes += entries[end].size;
                end += 1;
            }
            let batch = &entries[start..end];
            start = end;
            let input: String = batch
                .iter()
                .map(|entry| format!("{}\n", entry.oid))
                .collect();
            let objects = self.git(
                Path::new(&repo.path),
                &["cat-file", "--batch"],
                Some(input.as_bytes()),
            )?;
            let mut cursor = 0;
            for entry in batch {
                let Some(offset) = objects
                    .get(cursor..)
                    .and_then(|rest| rest.iter().position(|&byte| byte == b'\n'))
                else {
                    return Err(reason(OBJECT_CHANGED));
                };
                let newline = cursor + offset;
                let header = format!("{} blob {}", entry.oid, entry.size);
                if &objects[cursor..newline] != header.as_bytes() {
                    return Err(reason(OBJECT_CHANGED));
                }
                let path = safe_path(work_path, &entry.path)?;
                cursor = newline + 1;
                let body_end = cursor + entry.size as usize;
                if body_end >= objects.len() || objects[body_end] != b'\n' {
                    return Err(reason("Incomplete Git object."));
                }
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&path, &objects[cursor..body_end])?;
                cursor = body_end + 1;
            }
        }
        Ok(())
    }

    /// `create` (`worktrees.ts:145`): adds the managed worktree on a new (or existing) branch with no checkout, fills
    /// it from the objects, and writes the owner marker last (`wx`, so an existing marker is never replaced).
    pub fn create(
        &self,
        repo: &Repository,
        work: &ManagedWorktree,
        existing_branch: bool,
    ) -> Result<(), RpcError> {
        self.assert_repository(repo)?;
        self.validate_ref(repo, &work.branch)?;
        let expected = self.describe(&work.id, &work.base_sha, Some(work.branch.as_str()))?;
        let work_path = Path::new(&work.path);
        if !same_location(Path::new(&expected.path), work_path)? || work_path.exists() {
            return Err(reason("worktree_path_occupied"));
        }
        let entries = self.entries(repo, &work.base_sha)?;
        let name = work
            .branch
            .strip_prefix("refs/heads/")
            .unwrap_or(work.branch.as_str());
        let mut args = vec![
            "worktree",
            "add",
            "--no-checkout",
            "--lock",
            "--reason",
            "Tjommie managed workspace",
        ];
        if existing_branch {
            args.extend([work.path.as_str(), name]);
        } else {
            args.extend([
                "-b",
                name,
                work.path.as_str(),
                work.base_sha.as_str(),
            ]);
        }
        self.git(Path::new(&repo.path), &args, None)?;
        self.materialize(repo, work, &work.base_sha, &entries)?;
        self.write_owner(repo, work)
    }

    /// The owner marker write at the end of `create` (`worktrees.ts:153`).
    fn write_owner(&self, repo: &Repository, work: &ManagedWorktree) -> Result<(), RpcError> {
        let marker = serde_json::to_string(&OwnerMarker {
            id: work.id.as_str(),
            path: work.path.as_str(),
            branch: work.branch.as_str(),
            base_sha: work.base_sha.as_str(),
            common_dir: repo.common_dir.as_str(),
        })
        .expect("an owner marker is plain strings");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.owner_path(&work.id))?;
        file.write_all(marker.as_bytes())?;
        Ok(())
    }

    /// The owner marker path, `.owners/<id>.json` (`worktrees.ts:153`, `:161`, `:254`).
    fn owner_path(&self, id: &str) -> PathBuf {
        self.root.join(".owners").join(format!("{id}.json"))
    }

    /// `assertOwned` (`worktrees.ts:156`): the worktree is the one this daemon created, on its branch, under the
    /// repository it came from.
    pub fn assert_owned(&self, repo: &Repository, work: &ManagedWorktree) -> Result<(), RpcError> {
        self.assert_repository(repo)?;
        let expected = self.describe(&work.id, &work.base_sha, Some(work.branch.as_str()))?;
        let work_path = Path::new(&work.path);
        if !same_location(Path::new(&expected.path), work_path)?
            || is_link(&fs::symlink_metadata(work_path)?)
            || !same_location(&real_path(work_path)?, work_path)?
        {
            return Err(reason("worktree_ownership_changed"));
        }
        let owner = fs::read_to_string(self.owner_path(&work.id))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .ok_or_else(|| reason("worktree_ownership_missing"))?;
        let Some(owner) = owner.as_object() else {
            return Err(reason("worktree_ownership_changed"));
        };
        if owner.get("commonDir").and_then(Value::as_str) != Some(repo.common_dir.as_str())
            || owner.get("branch").and_then(Value::as_str) != Some(work.branch.as_str())
        {
            return Err(reason("worktree_ownership_changed"));
        }
        let text = self.text(
            work_path,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-common-dir",
                "--symbolic-full-name",
                "HEAD",
            ],
        )?;
        let mut lines = text.lines();
        let common = real_path(Path::new(lines.next().unwrap_or_default()))?;
        let branch = lines.next();
        if !same_location(&common, Path::new(&repo.common_dir))?
            || branch != Some(work.branch.as_str())
        {
            return Err(reason("worktree_ownership_changed"));
        }
        Ok(())
    }

    /// `head` (`worktrees.ts:168`): the commit the owned worktree has checked out.
    pub fn head(&self, repo: &Repository, work: &ManagedWorktree) -> Result<String, RpcError> {
        self.assert_owned(repo, work)?;
        commit_sha(&self.text(Path::new(&work.path), &["rev-parse", "HEAD"])?)
    }

    /// `dirty` (`worktrees.ts:170`): whether the owned worktree differs from `sha` (default: its HEAD) in its index,
    /// its untracked files (unless `allow_untracked`) or its file bytes.
    pub fn dirty(
        &self,
        repo: &Repository,
        work: &ManagedWorktree,
        sha: Option<&str>,
        allow_untracked: bool,
    ) -> Result<bool, RpcError> {
        let sha = match sha {
            Some(sha) => sha.to_owned(),
            None => self.head(repo, work)?,
        };
        self.assert_owned(repo, work)?;
        let work_path = Path::new(&work.path);
        let tree = format!("{sha}^{{tree}}");
        if self.text(work_path, &["rev-parse", "HEAD"])? != sha
            || self.text(work_path, &["write-tree"])?
                != self.text(Path::new(&repo.path), &["rev-parse", tree.as_str()])?
        {
            return Ok(true);
        }
        if !allow_untracked
            && !self
                .git(
                    work_path,
                    &["ls-files", "--others", "--exclude-standard", "-z"],
                    None,
                )?
                .is_empty()
        {
            return Ok(true);
        }
        for entry in self.entries(repo, &sha)? {
            let path = safe_path(work_path, &entry.path)?;
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                return Ok(true);
            };
            if !metadata.is_file() || is_link(&metadata) {
                return Ok(true);
            }
            let Ok(bytes) = fs::read(&path) else {
                return Ok(true);
            };
            if blob_id(&sha, &bytes) != entry.oid {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `diff(base, sha)` (`worktrees.ts:186`): the patch between two commits, cut at 512 KiB.
    pub fn diff(&self, repo: &Repository, base: &str, sha: &str) -> Result<DiffText, RpcError> {
        commit_sha(base)?;
        commit_sha(sha)?;
        let bytes = self.git(
            Path::new(&repo.path),
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--unified=3",
                base,
                sha,
                "--",
            ],
            None,
        )?;
        let shown = bytes.len().min(DIFF_LIMIT);
        Ok(DiffText {
            text: String::from_utf8_lossy(&bytes[..shown]).into_owned(),
            truncated: bytes.len() > DIFF_LIMIT,
        })
    }

    /// `changedFiles` (`worktrees.ts:192`): every path that differs from `sha` in the owned worktree, in first-seen
    /// order. A file that cannot be read or checked counts as changed.
    pub fn changed_files(
        &self,
        repo: &Repository,
        work: &ManagedWorktree,
        sha: &str,
    ) -> Result<Vec<String>, RpcError> {
        self.assert_owned(repo, work)?;
        commit_sha(sha)?;
        let work_path = Path::new(&work.path);
        let mut changed: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut add = |path: String| {
            if seen.insert(path.clone()) {
                changed.push(path);
            }
        };
        let staged = self.git(
            work_path,
            &[
                "diff",
                "--cached",
                "--name-only",
                "--no-ext-diff",
                "--no-textconv",
                "-z",
                sha,
                "--",
            ],
            None,
        )?;
        for path in nul_separated(&staged) {
            add(path);
        }
        let others = self.git(work_path, &["ls-files", "--others", "-z"], None)?;
        for path in nul_separated(&others) {
            add(path);
        }
        for entry in self.entries(repo, sha)? {
            let read = safe_path(work_path, &entry.path)
                .and_then(|path| fs::read(path).map_err(RpcError::from));
            match read {
                Ok(bytes) if blob_id(sha, &bytes) == entry.oid => {}
                _ => add(entry.path),
            }
        }
        Ok(changed)
    }

    /// `mergeCandidate` (`worktrees.ts:206`): merges an accepted commit into `target` inside the owned candidate
    /// worktree. A clean merge becomes a commit on the candidate's branch; a conflict leaves the markers in the files.
    pub fn merge_candidate(
        &self,
        repo: &Repository,
        work: &ManagedWorktree,
        target: &str,
        accepted: &str,
    ) -> Result<MergeCandidate, RpcError> {
        commit_sha(target)?;
        commit_sha(accepted)?;
        let repo_path = Path::new(&repo.path);
        // Exit status 1 means "conflicts" (a meaningful answer); anything else is unavailable.
        let merge = match self.run(
            repo_path,
            &[
                "merge-tree",
                "--write-tree",
                "--name-only",
                target,
                accepted,
            ],
            None,
            MERGE_OUTPUT_LIMIT,
        ) {
            Ok(merge) if matches!(merge.code, Some(0 | 1)) && !merge.exceeded => merge,
            _ => return Err(reason(MERGE_UNAVAILABLE)),
        };
        let stdout = String::from_utf8_lossy(&merge.stdout).into_owned();
        let text: &str = &stdout;
        // `split(/\r?\n/)[0]`: the first line, without a CR that precedes its newline.
        let first_line = match text.find('\n') {
            Some(end) => text[..end].strip_suffix('\r').unwrap_or(&text[..end]),
            None => text,
        };
        let tree = commit_sha(first_line)?;
        self.create(repo, work, false)?;
        if merge.code == Some(1) {
            self.materialize(repo, work, &tree, &self.entries(repo, &tree)?)?;
            return Ok(MergeCandidate {
                sha: None,
                conflict: true,
            });
        }
        let message = format!("Integrate accepted Tjommie task {}\n", work.id);
        let commit = self.git(
            repo_path,
            &["commit-tree", tree.as_str(), "-p", target, "-p", accepted],
            Some(message.as_bytes()),
        )?;
        let sha = commit_sha(String::from_utf8_lossy(&commit).trim())?;
        self.materialize(repo, work, &sha, &self.entries(repo, &sha)?)?;
        self.git(
            Path::new(&work.path),
            &["update-ref", "HEAD", sha.as_str(), target],
            None,
        )?;
        Ok(MergeCandidate {
            sha: Some(sha),
            conflict: false,
        })
    }

    /// `promote` (`worktrees.ts:223`): moves a branch the user does not have checked out to `candidate`, through an
    /// owned target worktree. Returns the stop reason instead of an error when the target cannot move safely.
    pub fn promote(
        &self,
        repo: &Repository,
        target_ref: &str,
        expected: &str,
        candidate: &str,
    ) -> Result<Promotion, RpcError> {
        if self.resolve_ref(repo, target_ref)? != expected {
            return Ok(stopped(None, "target_head_moved"));
        }
        commit_sha(candidate)?;
        let digest = format!(
            "{:x}",
            Sha256::digest(format!("{}{}", repo.common_dir, target_ref))
        );
        let id = format!("target-{}", &digest[..32]);
        let work = self.describe(&id, expected, Some(target_ref))?;
        let work_path = Path::new(&work.path);
        let repo_path = Path::new(&repo.path);
        let records = self.git(
            repo_path,
            &["worktree", "list", "--porcelain", "-z"],
            None,
        )?;
        let records = String::from_utf8_lossy(&records).into_owned();
        let branch = format!("branch {target_ref}");
        for record in records.split("\0\0") {
            let fields: Vec<&str> = record.split('\0').collect();
            if !fields.contains(&branch.as_str()) {
                continue;
            }
            let mut owned = false;
            for field in &fields {
                if let Some(path) = field.strip_prefix("worktree ")
                    && same_location(Path::new(&path.replace('/', "\\")), work_path)?
                {
                    owned = true;
                    break;
                }
            }
            if !owned {
                return Ok(stopped(None, "target_checked_out"));
            }
        }
        if !work_path.exists() {
            self.create(repo, &work, true)?;
        }
        self.assert_owned(repo, &work)?;
        if self.head(repo, &work)? != expected {
            return Ok(stopped(Some(work_path), "target_head_moved"));
        }
        if self.dirty(repo, &work, Some(expected), false)?
            || !self.changed_files(repo, &work, expected)?.is_empty()
        {
            return Ok(stopped(Some(work_path), "promotion_worktree_dirty"));
        }
        let ancestor = self.run(
            repo_path,
            &["merge-base", "--is-ancestor", expected, candidate],
            None,
            STATUS_OUTPUT_LIMIT,
        );
        if !ancestor.is_ok_and(|output| output.code == Some(0) && !output.exceeded) {
            return Ok(stopped(Some(work_path), "target_not_ancestor"));
        }
        // Only the owned target index is materialized; the user's checked-out branch is never advanced.
        self.materialize(repo, &work, candidate, &self.entries(repo, candidate)?)?;
        self.git(
            work_path,
            &["update-ref", "HEAD", candidate, expected],
            None,
        )?;
        Ok(Promotion {
            path: Some(work.path.clone()),
            reason: None,
        })
    }

    /// `remove` (`worktrees.ts:246`): removes an owned worktree that still has exactly `expected_sha` and no changed
    /// files, unless a checkout filter would have to run to clean it up. Then drops its owner marker.
    pub fn remove(
        &self,
        repo: &Repository,
        work: &ManagedWorktree,
        expected_sha: &str,
    ) -> Result<(), RpcError> {
        self.assert_owned(repo, work)?;
        if self.head(repo, work)? != expected_sha
            || !self.changed_files(repo, work, expected_sha)?.is_empty()
        {
            return Err(reason(
                "The worktree has changed. Preserve its files before cleanup.",
            ));
        }
        let work_path = Path::new(&work.path);
        let tracked = String::from_utf8_lossy(&self.git(work_path, &["ls-files", "-z"], None)?)
            .into_owned();
        let attributes = self.git(
            work_path,
            &["check-attr", "--cached", "-z", "--stdin", "filter"],
            Some(tracked.as_bytes()),
        )?;
        for value in String::from_utf8_lossy(&attributes)
            .split('\0')
            .skip(2)
            .step_by(3)
        {
            if !value.is_empty() && !matches!(value, "unspecified" | "unset") {
                return Err(reason(
                    "Automatic cleanup of a filtered checkout is unavailable. Its files and branch are retained.",
                ));
            }
        }
        let repo_path = Path::new(&repo.path);
        self.git(
            repo_path,
            &["worktree", "unlock", work.path.as_str()],
            None,
        )?;
        self.git(
            repo_path,
            &["worktree", "remove", work.path.as_str()],
            None,
        )?;
        let marker = self.owner_path(&work.id);
        if is_within(&marker, &self.root)? {
            fs::remove_file(&marker)?;
        }
        Ok(())
    }

    /// The git invocation shared by every call: the protections first, then the caller's arguments
    /// (`worktrees.ts:36`).
    fn arguments(&self, args: &[&str]) -> Vec<String> {
        // Git hooks and fsmonitor are code execution; accepted project checks run separately.
        let mut all = vec![
            "--no-replace-objects".to_owned(),
            "-c".to_owned(),
            format!("core.hooksPath={}", self.hooks.display()),
            "-c".to_owned(),
            "core.fsmonitor=false".to_owned(),
        ];
        all.extend(args.iter().map(|arg| (*arg).to_owned()));
        all
    }

    /// Runs git with the scrubbed environment and returns its exit code and stdout. Input is written from its own
    /// thread while stdout and stderr are drained, so a large input cannot deadlock against a full pipe. A spawn
    /// failure is an `io::Error`; a non-zero exit is not.
    fn run(
        &self,
        path: &Path,
        args: &[&str],
        input: Option<&[u8]>,
        limit: u64,
    ) -> io::Result<GitOutput> {
        let mut command = spawn::command("git");
        command
            .args(self.arguments(args))
            .current_dir(path)
            .env_clear()
            .envs(scrubbed_environment())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let (stdin, stdout, stderr) = (
            child.stdin.take(),
            child.stdout.take(),
            child.stderr.take(),
        );
        thread::scope(|scope| -> io::Result<GitOutput> {
            let writer = scope.spawn(move || match (stdin, input) {
                (Some(mut stdin), Some(bytes)) => stdin.write_all(bytes),
                _ => Ok(()),
            });
            let out = scope.spawn(move || read_capped(stdout, limit));
            let err = scope.spawn(move || read_capped(stderr, limit));
            let code = child.wait()?.code();
            writer
                .join()
                .map_err(|_| io::Error::other("git input thread panicked"))??;
            let (stdout, stdout_over) = out
                .join()
                .map_err(|_| io::Error::other("git output thread panicked"))??;
            let (_, stderr_over) = err
                .join()
                .map_err(|_| io::Error::other("git error thread panicked"))??;
            Ok(GitOutput {
                code,
                stdout,
                exceeded: stdout_over || stderr_over,
            })
        })
    }
}

/// `reason(message)` (`worktrees.ts:15`): a `bad_state` error with the message as given.
fn reason(message: &str) -> RpcError {
    RpcError::new(ErrorCode::BadState, message)
}

/// `#env()` (`worktrees.ts:28`): only these variables reach git.
fn scrubbed_environment() -> Vec<(&'static str, OsString)> {
    ENV_KEYS
        .into_iter()
        .filter_map(|key| env::var_os(key).map(|value| (key, value)))
        .collect()
}

/// What a pipe carries, up to `limit` bytes, and whether it carried more. The excess is drained and dropped, so memory
/// stays bounded and the child never blocks on a full pipe. A missing pipe carries nothing.
fn read_capped(pipe: Option<impl Read>, limit: u64) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut over = false;
    if let Some(mut pipe) = pipe {
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let count = pipe.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            let room = usize::try_from(limit.saturating_sub(bytes.len() as u64)).unwrap_or(usize::MAX);
            if count > room {
                over = true;
            }
            bytes.extend_from_slice(&chunk[..count.min(room)]);
        }
    }
    Ok((bytes, over))
}

/// `equalPath` (`worktrees.ts:13`) for two lexical positions on one walk: case-insensitive, like `toLowerCase`.
fn same_spelling(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// `lstat(path).isSymbolicLink()` (`worktrees.ts:139`), widened to every reparse point: a symlink, a junction, or
/// another reparse point that Node would report as a plain file.
fn is_link(metadata: &fs::Metadata) -> bool {
    metadata.is_symlink() || (metadata.file_attributes() & REPARSE_POINT) != 0
}

/// `#safePath` (`worktrees.ts:135`): `name` under `root`, refusing anything outside it, the root itself, and any
/// link on the way from the file up to the root.
fn safe_path(root: &Path, name: &str) -> Result<PathBuf, RpcError> {
    let path = root.join(name);
    if !is_within(&path, root)? || same_location(root, &path)? {
        return Err(reason(UNSUPPORTED_PATH));
    }
    // A lexical walk up to the root, as in the TypeScript: an identity match would stop before a junction is seen.
    let mut folder = Some(path.as_path());
    while let Some(current) = folder.filter(|current| !same_spelling(current, root)) {
        match fs::symlink_metadata(current) {
            Ok(metadata) if is_link(&metadata) => {
                return Err(reason("worktree_link_changed"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        folder = current.parent();
    }
    Ok(path)
}

/// The per-component checks of `#entries` (`worktrees.ts:93`).
fn unsafe_name(part: &str) -> bool {
    matches!(part.to_lowercase().as_str(), "" | "." | ".." | ".git")
        || GIT_SHORT_NAME.is_match(part)
        || INVALID_NAME_CHARS.is_match(part)
        || TRAILING_DOT_OR_SPACE.is_match(part)
        || RESERVED_NAME.is_match(part)
}

/// The blob id that `dirty` and `changedFiles` compare with (`worktrees.ts:174`): `blob N\0` plus the bytes, hashed
/// with SHA-256 for a 64-digit object format and SHA-1 otherwise.
fn blob_id(sha: &str, bytes: &[u8]) -> String {
    let header = format!("blob {}\0", bytes.len());
    if sha.len() == 64 {
        format!(
            "{:x}",
            Sha256::new().chain_update(&header).chain_update(bytes).finalize()
        )
    } else {
        format!(
            "{:x}",
            Sha1::new().chain_update(&header).chain_update(bytes).finalize()
        )
    }
}

/// `split("\0").filter(Boolean)` over a git listing.
fn nul_separated(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .split('\0')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A `promote` answer that stops with `reason_code` (`worktrees.ts:223`).
fn stopped(path: Option<&Path>, reason_code: &str) -> Promotion {
    Promotion {
        path: path.map(path_text),
        reason: Some(reason_code.to_owned()),
    }
}
