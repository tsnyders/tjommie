//! Port of `packages/daemon/src/project-review.test.ts`. The git fixture is the part of `projectFixture` this file uses:
//! a repository with committed files, a managed-worktree root, and the registered repository.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use daemon::ids::{new_id, now_iso};
use daemon::project_review::{
    CLAUDE_REVIEW_FLAGS, ReviewContext, ReviewInstructions, ReviewSnapshot, frozen_review_context,
    frozen_review_input, review_coverage, review_hash, review_instructions_hash, review_snapshot,
};
use daemon::project_review_policy::{
    ReviewEffectivePolicy, review_cli_args, review_effective_policy, review_environment,
};
use daemon::spawn;
use daemon::worktrees::{Repository, Worktrees};
use daemon::project_review_runtime::validate_review_reply;
use protocol::projects::EvaluationState;
use serde_json::{Value, json};
use tempfile::TempDir;

const EXAMPLE_PATH: &str = "src/example.ts";
const EXAMPLE_TEXT: &str = "export const example = 1;\n";
const EXAMPLE_DIFF: &str = "+return 1;";

/// The snapshot of the TypeScript tests: one file, a one-line diff, and a 40-character source SHA.
fn example_snapshot() -> ReviewSnapshot {
    snapshot_with(&[EXAMPLE_PATH], EXAMPLE_TEXT.into(), EXAMPLE_DIFF.into())
}

/// A snapshot whose every listed file has the same text.
fn snapshot_with(files: &[&str], text: String, diff: String) -> ReviewSnapshot {
    let contents: BTreeMap<String, String> = files
        .iter()
        .map(|path| (path.to_string(), text.clone()))
        .collect();
    ReviewSnapshot::new(
        "a".repeat(40),
        files.iter().map(|path| path.to_string()).collect(),
        diff,
        contents,
    )
}

/// The environment the TypeScript fixture gives git: no system or global configuration, and fixed identities.
fn git_environment(empty_config: &Path) -> Vec<(String, String)> {
    let mut env = vec![
        ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
        (
            "GIT_CONFIG_GLOBAL".to_string(),
            empty_config.to_string_lossy().into_owned(),
        ),
        (
            "GIT_AUTHOR_NAME".to_string(),
            "Synthetic fixture".to_string(),
        ),
        (
            "GIT_AUTHOR_EMAIL".to_string(),
            "fixture@example.invalid".to_string(),
        ),
        (
            "GIT_COMMITTER_NAME".to_string(),
            "Synthetic fixture".to_string(),
        ),
        (
            "GIT_COMMITTER_EMAIL".to_string(),
            "fixture@example.invalid".to_string(),
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
        if let Some(value) = std::env::var_os(key) {
            env.push((key.to_string(), value.to_string_lossy().into_owned()));
        }
    }
    env
}

/// Runs git in `path` with the fixture's environment and returns its trimmed stdout.
fn git(path: &Path, env: &[(String, String)], args: &[&str]) -> String {
    let output = spawn::command("git")
        .args(args)
        .current_dir(path)
        .env_clear()
        .envs(env.iter().map(|(key, value)| (key, value)))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

struct Fixture {
    _root: TempDir,
    repo_path: PathBuf,
    env: Vec<(String, String)>,
    base: String,
    worktrees: Worktrees,
    repository: Repository,
}

/// `projectFixture` (project-test-support.ts), reduced to the repository and worktree root that these tests use.
fn project_fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let repo_path = root.path().join("owner");
    fs::create_dir(&repo_path).unwrap();
    let empty_config = root.path().join("empty-gitconfig");
    fs::write(&empty_config, "").unwrap();
    let env = git_environment(&empty_config);

    git(&repo_path, &env, &["init", "-b", "main"]);
    git(&repo_path, &env, &["config", "user.name", "Synthetic fixture"]);
    git(
        &repo_path,
        &env,
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(&repo_path, &env, &["config", "commit.gpgsign", "false"]);
    fs::create_dir(repo_path.join("src")).unwrap();
    fs::write(repo_path.join("src/a.txt"), "A0\n").unwrap();
    fs::write(repo_path.join("src/b.txt"), "B0\n").unwrap();
    fs::write(repo_path.join(".gitignore"), "check-output.txt\n").unwrap();
    fs::write(
        repo_path.join("check.mjs"),
        r#"import{writeFileSync}from"node:fs";writeFileSync("check-output.txt","passed");"#,
    )
    .unwrap();
    git(&repo_path, &env, &["add", "."]);
    git(&repo_path, &env, &["commit", "-m", "Synthetic base"]);
    let base = git(&repo_path, &env, &["rev-parse", "HEAD"]);

    let managed = root.path().join("managed");
    fs::create_dir(&managed).unwrap();
    let worktrees = Worktrees::new(&managed).unwrap();
    let info = worktrees
        .repository(&repo_path.to_string_lossy())
        .unwrap();
    let repository = Repository {
        id: new_id(),
        path: info.path,
        common_dir: info.common_dir,
        name: info.name,
        created_at: now_iso(),
    };
    Fixture {
        _root: root,
        repo_path,
        env,
        base,
        worktrees,
        repository,
    }
}

#[test]
fn review_hash_matches_the_published_sha256_vectors() {
    assert_eq!(
        review_hash(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        review_hash(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn freezes_the_trusted_criteria_and_binds_every_instruction_and_record_revision() {
    let mut instructions = ReviewInstructions {
        reviewer: "Check correctness".into(),
        role: "Review this team".into(),
        title: "Deliver source".into(),
        task: "Implement the task".into(),
        acceptance: vec!["Preserve exact source bytes".into()],
    };
    let context = ReviewContext {
        bot_id: "reviewer".into(),
        project_id: "project".into(),
        task_id: "task".into(),
        attempt_id: "attempt".into(),
        evaluation_id: "evaluation".into(),
        revision: 2,
        project_revision: 3,
        task_revision: 4,
        instructions: instructions.clone(),
    };
    let frozen = frozen_review_context(context.clone()).unwrap();
    let before = review_instructions_hash(&frozen);

    // The caller's copy changes after dispatch; the frozen context must not.
    instructions.task = "Changed after dispatch".into();
    instructions.acceptance.push("Changed checklist".into());
    assert_eq!(frozen.instructions.task, "Implement the task");
    assert_eq!(
        frozen.instructions.acceptance,
        vec!["Preserve exact source bytes".to_string()]
    );

    let patches = [
        ReviewContext {
            revision: 3,
            ..frozen.clone()
        },
        ReviewContext {
            task_revision: 5,
            ..frozen.clone()
        },
        ReviewContext {
            project_revision: 4,
            ..frozen.clone()
        },
        ReviewContext {
            instructions: instructions.clone(),
            ..frozen.clone()
        },
    ];
    for patch in &patches {
        assert_ne!(review_instructions_hash(patch), before);
    }
    assert_eq!(
        frozen_review_context(ReviewContext {
            task_revision: 0,
            ..context
        })
        .unwrap_err(),
        "review_instruction_context_missing"
    );
}

#[test]
fn reviews_exact_git_objects_without_reading_changed_worktree_files_and_fails_closed_on_binary_coverage()
{
    let h = project_fixture();
    let frozen = review_snapshot(&h.worktrees, &h.repository, &h.base, &h.base).unwrap();
    let before = review_coverage(&frozen).unwrap();

    fs::write(h.repo_path.join("src/a.txt"), "uncommitted owner text\n").unwrap();
    assert_eq!(frozen.read("src/a.txt").unwrap(), "A0\n");
    assert_eq!(review_coverage(&frozen).unwrap(), before);
    assert!(frozen.read("../private").unwrap_err().contains("outside"));

    fs::write(h.repo_path.join("binary"), [1u8, 0, 2]).unwrap();
    git(&h.repo_path, &h.env, &["add", "binary"]);
    git(
        &h.repo_path,
        &h.env,
        &["commit", "-m", "Synthetic unsupported binary"],
    );
    let head = git(&h.repo_path, &h.env, &["rev-parse", "HEAD"]);
    let error = review_snapshot(&h.worktrees, &h.repository, &h.base, &head).unwrap_err();
    assert!(error.contains("coverage_incomplete"));
}

#[test]
fn bounds_and_hashes_the_whole_frozen_input_and_rejects_omitted_repeated_or_oversized_source() {
    let snapshot = example_snapshot();
    let (text, coverage) = frozen_review_input(&snapshot).unwrap();
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed["files"][0]["path"], EXAMPLE_PATH);
    assert_eq!(parsed["files"][0]["text"], EXAMPLE_TEXT);

    assert!(coverage.complete);
    assert_eq!(coverage.source_sha, snapshot.source_sha);
    assert_eq!(coverage.files[0].bytes, 26);
    assert_eq!(review_coverage(&snapshot).unwrap(), coverage);

    let repeated = snapshot_with(
        &[EXAMPLE_PATH, EXAMPLE_PATH],
        EXAMPLE_TEXT.into(),
        EXAMPLE_DIFF.into(),
    );
    assert!(
        frozen_review_input(&repeated)
            .unwrap_err()
            .contains("coverage_incomplete")
    );
    let oversized = snapshot_with(&[EXAMPLE_PATH], "x".repeat(262_145), EXAMPLE_DIFF.into());
    assert!(
        frozen_review_input(&oversized)
            .unwrap_err()
            .contains("coverage_incomplete")
    );
    let large_diff = snapshot_with(&[EXAMPLE_PATH], EXAMPLE_TEXT.into(), "x".repeat(524_289));
    assert!(
        frozen_review_input(&large_diff)
            .unwrap_err()
            .contains("coverage_incomplete")
    );
}

#[test]
fn frozen_input_text_is_the_exact_javascript_json() {
    let snapshot = example_snapshot();
    let (text, coverage) = frozen_review_input(&snapshot).unwrap();
    let expected = r#"{"sourceSha":"SHA","files":[{"path":"src/example.ts","text":"export const example = 1;\n"}],"diff":"+return 1;"}"#
        .replace("SHA", &"a".repeat(40));
    assert_eq!(text, expected);
    // TODO(M5-integration): confirm the golden hash against node. The TypeScript snapshotSha256 of this exact text
    // goes here as a literal; until then this only checks that the hash covers the text above.
    assert_eq!(coverage.snapshot_sha256, review_hash(expected.as_bytes()));
}

#[test]
fn json_escapes_exactly_as_javascript_json_stringify_does() {
    // Written without escape sequences in the source: `back` is a single backslash.
    let back = char::from(92u8);
    let separator = char::from_u32(0x2028).unwrap();
    let source = format!("say \"hi\"\tback{back}slash {}{separator}", char::from(1u8));
    let snapshot = snapshot_with(&["a.txt"], source, String::new());

    let quote = format!("{back}\"");
    let tab = format!("{back}t");
    let double = format!("{back}{back}");
    let control = format!("{back}u0001");
    let escaped = format!("say {quote}hi{quote}{tab}back{double}slash {control}{separator}");
    let sha = "a".repeat(40);
    let expected = format!(
        "{{\"sourceSha\":\"{sha}\",\"files\":[{{\"path\":\"a.txt\",\"text\":\"{escaped}\"}}],\"diff\":\"\"}}"
    );
    assert_eq!(frozen_review_input(&snapshot).unwrap().0, expected);
}

const INSTRUCTIONS_SHA: &str =
    "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

fn reply(snapshot: &ReviewSnapshot) -> Value {
    json!({
        "sourceSha": snapshot.source_sha,
        "snapshotSha256": review_coverage(snapshot).unwrap().snapshot_sha256,
        "instructionsSha256": INSTRUCTIONS_SHA,
        "complete": true,
        "reviewedFiles": snapshot.files,
        "state": "passed",
        "reason": null,
        "findings": [],
    })
}

/// `{ ...reply(), ...patch }`.
fn with_patch(mut value: Value, patch: Value) -> Value {
    let object = value.as_object_mut().unwrap();
    for (key, field) in patch.as_object().unwrap() {
        object.insert(key.clone(), field.clone());
    }
    value
}

#[test]
fn requires_complete_exact_source_coverage_and_confines_findings_to_real_snapshot_lines() {
    let snapshot = example_snapshot();
    let accepted =
        validate_review_reply(&reply(&snapshot).to_string(), &snapshot, INSTRUCTIONS_SHA).unwrap();
    assert_eq!(accepted.state, EvaluationState::Passed);
    assert_eq!(accepted.coverage, Some(review_coverage(&snapshot).unwrap()));

    let patches = [
        json!({"sourceSha": "b".repeat(40)}),
        json!({"snapshotSha256": "0".repeat(64)}),
        json!({"instructionsSha256": "f".repeat(64)}),
        json!({"complete": false}),
        json!({"reviewedFiles": []}),
        json!({"reviewedFiles": [EXAMPLE_PATH, EXAMPLE_PATH]}),
        json!({"findings": [{"severity": "high", "path": "../private", "line": 1, "message": "Outside"}]}),
        json!({"findings": [{"severity": "high", "path": EXAMPLE_PATH, "line": 99, "message": "Outside line"}]}),
    ];
    for patch in patches {
        let text = with_patch(reply(&snapshot), patch).to_string();
        assert!(validate_review_reply(&text, &snapshot, INSTRUCTIONS_SHA).is_err());
    }

    let findings = json!([{
        "severity": "high",
        "path": EXAMPLE_PATH,
        "line": 1,
        "message": "A concrete synthetic finding",
    }]);
    let text = with_patch(reply(&snapshot), json!({ "findings": findings })).to_string();
    assert_eq!(
        validate_review_reply(&text, &snapshot, INSTRUCTIONS_SHA)
            .unwrap()
            .state,
        EvaluationState::Findings
    );
}

/// `reason` and a finding's `line` are `.nullable()` in zod, not `.optional()`: `null` is fine, a missing key is not.
#[test]
fn requires_the_nullable_reason_and_line_keys_to_be_present() {
    let snapshot = example_snapshot();
    let mut no_reason = reply(&snapshot);
    no_reason.as_object_mut().unwrap().remove("reason");
    assert!(validate_review_reply(&no_reason.to_string(), &snapshot, INSTRUCTIONS_SHA).is_err());

    let no_line = json!([{"severity": "high", "path": EXAMPLE_PATH, "message": "No line key"}]);
    let text = with_patch(reply(&snapshot), json!({ "findings": no_line })).to_string();
    assert!(validate_review_reply(&text, &snapshot, INSTRUCTIONS_SHA).is_err());

    let null_line =
        json!([{"severity": "high", "path": EXAMPLE_PATH, "line": null, "message": "File-level finding"}]);
    let text = with_patch(reply(&snapshot), json!({ "findings": null_line })).to_string();
    assert_eq!(
        validate_review_reply(&text, &snapshot, INSTRUCTIONS_SHA)
            .unwrap()
            .state,
        EvaluationState::Findings
    );
}

#[test]
fn inspects_effective_tool_and_plugin_exposure_and_pins_a_tool_free_hook_free_api_policy() {
    let base = json!({"type": "system", "subtype": "init", "tools": [], "mcp_servers": [], "plugins": []});
    assert_eq!(
        review_effective_policy(&base).unwrap(),
        ReviewEffectivePolicy {
            tools: Vec::new(),
            mcp_servers: Vec::new(),
            plugins: Vec::new(),
        }
    );
    let patches = [
        json!({"tools": ["Bash"]}),
        // `{ tools: undefined }` in TypeScript: the key is present but not an array.
        json!({"tools": null}),
        json!({"mcp_servers": [{"name": "team"}]}),
        json!({"plugins": [{"name": "unknown", "path": "builtin", "source": "unknown@builtin"}]}),
    ];
    for patch in patches {
        let value = with_patch(base.clone(), patch);
        assert!(
            review_effective_policy(&value)
                .unwrap_err()
                .contains("unavailable")
        );
    }

    let args = review_cli_args("test", "owned-settings.json", Some(0.02)).unwrap();
    assert!(args.contains(&"--bare".to_string()));
    assert!(args.contains(&"--safe-mode".to_string()));
    for flag in CLAUDE_REVIEW_FLAGS {
        assert!(args.contains(&flag.to_string()));
    }
    assert_eq!(flag_value(&args, "--tools"), "");
    assert_eq!(flag_value(&args, "--setting-sources"), "");
    assert!(!args.contains(&"--resume".to_string()));
    assert_eq!(flag_value(&args, "--max-budget-usd"), "0.02");
    assert!(
        review_cli_args("test", "owned-settings.json", Some(0.0))
            .unwrap_err()
            .contains("budget")
    );

    let root = tempfile::tempdir().unwrap();
    let env = review_environment(root.path()).unwrap();
    assert!(!env.contains_key("ANTHROPIC_API_KEY"));
    assert!(!env.contains_key("OPENAI_API_KEY"));
    assert_ne!(
        env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        std::env::var("CLAUDE_CONFIG_DIR").ok().as_deref()
    );
    assert_ne!(
        env.get("HOME").map(String::as_str),
        std::env::var("HOME").ok().as_deref()
    );
}

/// The value after `flag` in a CLI argument list.
fn flag_value<'a>(args: &'a [String], flag: &str) -> &'a str {
    let position = args.iter().position(|arg| arg == flag).unwrap();
    &args[position + 1]
}
