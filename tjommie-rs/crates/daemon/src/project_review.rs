//! Port of `packages/daemon/src/project-review.ts`: the immutable review snapshot, its coverage, the frozen review input
//! and the hashes that bind them.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use protocol::projects::{CoveredFile, EvaluationState, ReviewCoverage, TaskFinding};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::cli::BoxFuture;
use crate::project_checks::CheckError;
use crate::worktrees::{Repository, Worktrees};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const ACCEPTANCE_LIMIT: usize = 50;
const INSTRUCTIONS_BYTES_LIMIT: usize = 1_048_576;
const FILE_COUNT_LIMIT: usize = 2048;
/// In UTF-16 code units, as JavaScript's `path.length` counts them.
const PATH_UNITS_LIMIT: usize = 2000;
const FILE_BYTES_LIMIT: usize = 262_144;
const TOTAL_BYTES_LIMIT: usize = 2_097_152;
const DIFF_BYTES_LIMIT: usize = 524_288;
const SERIALIZED_BYTES_LIMIT: usize = 4_194_304;

const OUTSIDE_SNAPSHOT: &str = "The file is outside the immutable review snapshot.";
const FILE_COUNT: &str = "review_coverage_incomplete: file count";
const FILE_PATH: &str = "review_coverage_incomplete: file path";
const SOURCE_SIZE_OR_ENCODING: &str = "review_coverage_incomplete: source size or encoding";
const DIFF_SIZE: &str = "review_coverage_incomplete: diff size";
const SERIALIZED_SIZE: &str = "review_coverage_incomplete: serialized source size";
const UNSUPPORTED_FILE_NAME: &str = "review_coverage_incomplete: unsupported file name encoding";
const SYMLINK_OR_SUBMODULE: &str = "review_coverage_incomplete: symlink or submodule";
const SOURCE_SIZE_LIMIT: &str = "review_coverage_incomplete: source size limit";
const BINARY_FILE: &str = "review_coverage_incomplete: binary file";
const UNSUPPORTED_TEXT: &str = "review_coverage_incomplete: unsupported text encoding";
const DIFF_SIZE_LIMIT: &str = "review_coverage_incomplete: diff size limit";
const INSTRUCTIONS_MISSING: &str = "review_instruction_context_missing";
const INSTRUCTIONS_LIMIT: &str = "review_instruction_context_limit";

// These flags are a candidate policy; no provider is enabled until a local acceptance probe establishes enforcement.
/// `CLAUDE_REVIEW_FLAGS` (project-review.ts:68).
pub const CLAUDE_REVIEW_FLAGS: [&str; 11] = [
    "--restricted",
    "--safe-mode",
    "--tools",
    "",
    "--strict-mcp-config",
    "--disable-slash-commands",
    "--setting-sources",
    "",
    "--permission-prompts",
    "none",
    "--no-session-persistence",
];

/// `ReviewSnapshot` (project-review.ts:6). The files are the exact Git objects of `source_sha`; `read` answers only for
/// those files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSnapshot {
    pub source_sha: String,
    pub files: Vec<String>,
    pub diff: String,
    contents: BTreeMap<String, String>,
}

impl ReviewSnapshot {
    /// Builds a snapshot from its parts. `contents` holds the decoded text of each file, keyed by path.
    pub fn new(
        source_sha: String,
        files: Vec<String>,
        diff: String,
        contents: BTreeMap<String, String>,
    ) -> Self {
        Self {
            source_sha,
            files,
            diff,
            contents,
        }
    }

    /// `snapshot.read(path)` (project-review.ts:6, 61-64).
    pub fn read(&self, path: &str) -> Result<&str, String> {
        self.contents
            .get(path)
            .map(String::as_str)
            .ok_or_else(|| OUTSIDE_SNAPSHOT.to_string())
    }
}

/// `ReviewInstructions` (project-review.ts:9). The fields are in the TypeScript key order, which `Serialize` keeps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewInstructions {
    pub reviewer: String,
    pub role: String,
    pub title: String,
    pub task: String,
    pub acceptance: Vec<String>,
}

/// `ReviewContext` (project-review.ts:7-9). The audit record serializes it as TypeScript does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewContext {
    pub bot_id: String,
    pub project_id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub evaluation_id: String,
    pub revision: u64,
    pub project_revision: u64,
    pub task_revision: u64,
    pub instructions: ReviewInstructions,
}

/// `ReviewResult` (project-review.ts:10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewResult {
    pub state: EvaluationState,
    pub reason: Option<String>,
    pub findings: Vec<TaskFinding>,
    pub coverage: Option<ReviewCoverage>,
}

/// `ReviewRuntime` (project-review.ts:11).
pub trait ReviewRuntime: Send + Sync {
    fn review<'a>(
        &'a self,
        snapshot: &'a ReviewSnapshot,
        signal: &'a CancellationToken,
        context: &'a ReviewContext,
    ) -> BoxFuture<'a, Result<ReviewResult, CheckError>>;
}

/// `reviewHash` (project-review.ts:12): SHA-256 of the bytes, as lowercase hex.
pub fn review_hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `JSON.stringify` for these values: compact, keys in insertion order. serde_json escapes the same characters as
/// JavaScript: quote, backslash and control characters only; non-ASCII is written as is.
pub(crate) fn js_stringify(value: &Value) -> String {
    serde_json::to_string(value).expect("a JSON value always serializes")
}

/// The instructions object in the TypeScript key order (reviewer, role, title, task, acceptance).
fn instructions_value(instructions: &ReviewInstructions) -> Value {
    json!({
        "reviewer": instructions.reviewer,
        "role": instructions.role,
        "title": instructions.title,
        "task": instructions.task,
        "acceptance": instructions.acceptance,
    })
}

/// `frozenReviewContext` (project-review.ts:13-19). The context is taken by value, so the caller's later changes can't
/// reach it (the TypeScript copies and freezes).
pub fn frozen_review_context(context: ReviewContext) -> Result<ReviewContext, String> {
    let safe = |value: u64| (1..=MAX_SAFE_INTEGER).contains(&value);
    if ![
        context.project_revision,
        context.task_revision,
        context.revision,
    ]
    .into_iter()
    .all(safe)
        || context.instructions.acceptance.len() > ACCEPTANCE_LIMIT
    {
        return Err(INSTRUCTIONS_MISSING.to_string());
    }
    if js_stringify(&instructions_value(&context.instructions)).len() > INSTRUCTIONS_BYTES_LIMIT {
        return Err(INSTRUCTIONS_LIMIT.to_string());
    }
    Ok(context)
}

/// `reviewInstructionsHash` (project-review.ts:20).
pub fn review_instructions_hash(context: &ReviewContext) -> String {
    let value = json!({
        "projectRevision": context.project_revision,
        "taskRevision": context.task_revision,
        "revision": context.revision,
        "instructions": instructions_value(&context.instructions),
    });
    review_hash(js_stringify(&value).as_bytes())
}

/// `frozenReviewInput` (project-review.ts:22-38): the exact JSON text the reviewer receives, and its coverage.
pub fn frozen_review_input(snapshot: &ReviewSnapshot) -> Result<(String, ReviewCoverage), String> {
    let distinct = snapshot.files.iter().collect::<HashSet<_>>().len();
    if snapshot.files.len() > FILE_COUNT_LIMIT || distinct != snapshot.files.len() {
        return Err(FILE_COUNT.to_string());
    }
    let mut bytes = 0usize;
    let mut files = Vec::with_capacity(snapshot.files.len());
    for path in &snapshot.files {
        if path.is_empty() || path.encode_utf16().count() > PATH_UNITS_LIMIT {
            return Err(FILE_PATH.to_string());
        }
        let text = snapshot.read(path)?;
        bytes += text.len();
        if text.len() > FILE_BYTES_LIMIT || bytes > TOTAL_BYTES_LIMIT || text.contains('\0') {
            return Err(SOURCE_SIZE_OR_ENCODING.to_string());
        }
        files.push((path.as_str(), text));
    }
    if snapshot.diff.len() > DIFF_BYTES_LIMIT {
        return Err(DIFF_SIZE.to_string());
    }
    let file_values: Vec<Value> = files
        .iter()
        .map(|(path, text)| json!({ "path": path, "text": text }))
        .collect();
    let text = js_stringify(&json!({
        "sourceSha": snapshot.source_sha,
        "files": file_values,
        "diff": snapshot.diff,
    }));
    if text.len() > SERIALIZED_BYTES_LIMIT {
        return Err(SERIALIZED_SIZE.to_string());
    }
    let coverage = ReviewCoverage {
        source_sha: snapshot.source_sha.clone(),
        snapshot_sha256: review_hash(text.as_bytes()),
        diff_sha256: review_hash(snapshot.diff.as_bytes()),
        complete: true,
        files: files
            .iter()
            .map(|(path, text)| CoveredFile {
                path: (*path).to_string(),
                bytes: text.len() as u64,
                sha256: review_hash(text.as_bytes()),
            })
            .collect(),
    };
    Ok((text, coverage))
}

/// `reviewCoverage` (project-review.ts:40).
pub fn review_coverage(snapshot: &ReviewSnapshot) -> Result<ReviewCoverage, String> {
    frozen_review_input(snapshot).map(|(_, coverage)| coverage)
}

/// `reviewSnapshot` (project-review.ts:42-65). Reads the exact Git objects of `source_sha`, never the worktree.
pub fn review_snapshot(
    worktrees: &Worktrees,
    repo: &Repository,
    base: &str,
    source_sha: &str,
) -> Result<ReviewSnapshot, String> {
    let repo_path = Path::new(&repo.path);
    // Every failure of the listing, git or decoding alike, reports the file-name error (project-review.ts:47-48).
    let listing = worktrees
        .git(
            repo_path,
            &["ls-tree", "-r", "-z", source_sha],
            None,
        )
        .map_err(|_| UNSUPPORTED_FILE_NAME.to_string())?;
    let tree = std::str::from_utf8(&listing).map_err(|_| UNSUPPORTED_FILE_NAME.to_string())?;

    let mut files = Vec::new();
    let mut contents = BTreeMap::new();
    let mut total = 0usize;
    for record in tree.split('\0').filter(|record| !record.is_empty()) {
        let (meta, path) = record
            .split_once('\t')
            .ok_or_else(|| SYMLINK_OR_SUBMODULE.to_string())?;
        let mut fields = meta.split(' ');
        let mode = fields.next().unwrap_or("");
        let kind = fields.next().unwrap_or("");
        let oid = fields.next().unwrap_or("");
        if kind != "blob" || !matches!(mode, "100644" | "100755") || oid.is_empty() {
            return Err(SYMLINK_OR_SUBMODULE.to_string());
        }

        let size_text = worktrees
            .git(repo_path, &["cat-file", "-s", oid], None)
            .map_err(|error| error.message)?;
        let size = parse_size(&size_text);
        total = total.saturating_add(size.unwrap_or(0));
        let within_limits = size.is_some_and(|size| size <= FILE_BYTES_LIMIT)
            && total <= TOTAL_BYTES_LIMIT
            && files.len() < FILE_COUNT_LIMIT;
        if !within_limits {
            return Err(SOURCE_SIZE_LIMIT.to_string());
        }

        let bytes = worktrees
            .git(repo_path, &["cat-file", "blob", oid], None)
            .map_err(|error| error.message)?;
        if bytes.contains(&0) {
            return Err(BINARY_FILE.to_string());
        }
        let text = decode_text(&bytes).ok_or_else(|| UNSUPPORTED_TEXT.to_string())?;
        files.push(path.to_string());
        contents.insert(path.to_string(), text);
    }

    let diff = worktrees
        .diff(repo, base, source_sha)
        .map_err(|error| error.message)?;
    if diff.truncated {
        return Err(DIFF_SIZE_LIMIT.to_string());
    }
    Ok(ReviewSnapshot::new(
        source_sha.to_string(),
        files,
        diff.text,
        contents,
    ))
}

/// `Number(text)` for `cat-file -s` output: surrounding whitespace is ignored, and empty text is 0.
fn parse_size(bytes: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        Some(0)
    } else {
        trimmed.parse().ok()
    }
}

/// `new TextDecoder("utf-8", { fatal: true }).decode`: strict UTF-8. The decoder drops a leading byte-order mark
/// (`ignoreBOM` is false by default), so the mark is dropped here too.
fn decode_text(bytes: &[u8]) -> Option<String> {
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    std::str::from_utf8(body).ok().map(str::to_owned)
}
