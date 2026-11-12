//! `project-test-support.ts`: a real Git repository and managed worktree root in a temp folder, a real database, four
//! bots, a project over them and a fake runner. Include with `mod common; mod project_support;`.
//!
//! Differences from the TypeScript fixture:
//! - The synthetic check is `cmd /c echo passed>check-output.txt` (the TypeScript runs `node check.mjs`), so it needs
//!   no Node. The file is git-ignored, as before.
//! - Turn endings settle on the Tokio runtime, so `complete` waits for the attempt to leave its running state
//!   (the TypeScript waits one `setImmediate`).
//! - Tests are plain `#[tokio::test]` (a single-threaded runtime): the services use `spawn_blocking` and the runtime
//!   handle, and a spawned task only runs when the test awaits, which stands in for the TypeScript's `setImmediate`.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use daemon::bots::{StoredBot, create_bot};
use daemon::cli::{BoxFuture, SystemCli};
use daemon::context::Db;
use daemon::project_checks::HostCheckRuntime;
use daemon::projects::{Projects, SqlRooms};
use daemon::queue::{WorkItem, WorkStatus, get_work_item, set_status};
use daemon::runner::{TurnOutcome, TurnResult};
use daemon::server::RpcError;
use daemon::spawn;
use daemon::tasks::{TaskRunner, Tasks, TasksOptions};
use daemon::worktrees::{Repository, Worktrees};
use protocol::model::BotInput;
use protocol::projects::{
    AttemptState, Project, ProjectInput, ProjectTask, TaskAttempt, TaskInput,
};
use serde_json::json;

use crate::common::{TempRoot, insert_account, make_default_account};

/// Runs `check` every 50 ms until it returns a value, for at most 30 s.
pub async fn eventually<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The `runner` of the fixture: delivers synchronously like the TypeScript fake.
pub struct FakeRunner {
    db: Db,
    pub delivered: Mutex<Vec<WorkItem>>,
    active: Mutex<HashMap<String, WorkItem>>,
    tasks: OnceLock<Weak<Tasks>>,
    /// `h.runner.retired = () => new Promise(() => {})`: while set, `retired` never resolves.
    pub hang_retired: AtomicBool,
}

impl FakeRunner {
    pub fn delivered(&self) -> Vec<WorkItem> {
        self.delivered.lock().unwrap().clone()
    }

    fn tasks(&self) -> Option<Arc<Tasks>> {
        self.tasks.get().and_then(Weak::upgrade)
    }

    /// Ends the item's turn the way the runner does, and tells the tasks.
    pub fn end_turn(&self, item: &WorkItem, status: WorkStatus, outcome: TurnOutcome) {
        let db = self.db.lock().unwrap();
        set_status(&db, &item.id, status).unwrap();
        self.active.lock().unwrap().remove(&item.bot_id);
        let result = TurnResult {
            outcome,
            text: Some("Completed".into()),
            text_message_id: None,
            error: None,
        };
        if let Some(tasks) = self.tasks() {
            tasks.turn_ended(&db, item, &result).unwrap();
        }
    }
}

impl TaskRunner for FakeRunner {
    fn deliver(&self, item: WorkItem) -> Result<WorkItem, RpcError> {
        self.delivered.lock().unwrap().push(item.clone());
        self.active.lock().unwrap().insert(item.bot_id.clone(), item.clone());
        let db = self.db.lock().unwrap();
        set_status(&db, &item.id, WorkStatus::Running)?;
        if let Some(tasks) = self.tasks() {
            tasks.turn_started(&db, &item)?;
        }
        Ok(item)
    }

    fn current_work(&self, bot_id: &str) -> Option<WorkItem> {
        self.active.lock().unwrap().get(bot_id).cloned()
    }

    fn stop_item(&self, item_id: &str, _notice: &str) -> BoxFuture<'static, Result<(), String>> {
        let item = get_work_item(&self.db.lock().unwrap(), item_id).map_err(|error| error.message);
        if let Ok(item) = item {
            self.end_turn(&item, WorkStatus::Interrupted, TurnOutcome::Interrupted);
        }
        Box::pin(async { Ok(()) })
    }

    fn retired(&self, _bot_id: &str) -> BoxFuture<'static, Result<(), String>> {
        if self.hang_retired.load(Ordering::SeqCst) {
            return Box::pin(std::future::pending());
        }
        Box::pin(async { Ok(()) })
    }
}

/// What `owner()` snapshots: the user's own checkout, which no task work may change.
#[derive(Debug, PartialEq, Eq)]
pub struct Owner {
    pub head: String,
    pub status: String,
    pub index: Vec<u8>,
}

pub struct Fixture {
    pub root: TempRoot,
    pub repo_path: PathBuf,
    pub base: String,
    pub db: Db,
    pub bots: Vec<StoredBot>,
    pub worktrees: Arc<Worktrees>,
    pub projects: Arc<Projects>,
    pub repository: Repository,
    pub project: Project,
    pub tasks: Arc<Tasks>,
    pub runner: Arc<FakeRunner>,
    git_env: Vec<(String, String)>,
}

fn git_env(root: &Path) -> Vec<(String, String)> {
    let config = root.join("empty-gitconfig");
    fs::write(&config, "").unwrap();
    let mut env = vec![
        ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
        ("GIT_CONFIG_GLOBAL".to_string(), config.to_string_lossy().into_owned()),
        ("GIT_AUTHOR_NAME".to_string(), "Synthetic fixture".to_string()),
        ("GIT_AUTHOR_EMAIL".to_string(), "fixture@example.invalid".to_string()),
        ("GIT_COMMITTER_NAME".to_string(), "Synthetic fixture".to_string()),
        ("GIT_COMMITTER_EMAIL".to_string(), "fixture@example.invalid".to_string()),
    ];
    for key in ["PATH", "SystemRoot", "SystemDrive", "WINDIR", "PATHEXT", "TEMP", "TMP", "COMSPEC"] {
        if let Ok(value) = std::env::var(key) {
            env.push((key.to_string(), value));
        }
    }
    env
}

/// `new Tasks(...)` options for a fixture: Full-auto testers run checks on the host through the real launcher.
pub fn host_options() -> TasksOptions {
    TasksOptions {
        host_checks: Some(Arc::new(HostCheckRuntime::new(Arc::new(SystemCli::new())))),
        ..TasksOptions::default()
    }
}

/// `projectFixture(options)`. Call inside a Tokio runtime.
pub fn fixture(options: TasksOptions) -> Fixture {
    let root = TempRoot::new();
    let repo_path = root.0.join("owner");
    let managed = root.0.join("managed");
    fs::create_dir(&repo_path).unwrap();
    let git_env = git_env(&root.0);
    let mut f = Fixture::bare(root, repo_path, managed, git_env, options);
    f.seed();
    f
}

impl Fixture {
    fn bare(
        root: TempRoot,
        repo_path: PathBuf,
        managed: PathBuf,
        git_env: Vec<(String, String)>,
        options: TasksOptions,
    ) -> Fixture {
        let db = daemon::db::open(&root.0.join("test.sqlite")).unwrap();
        let accounts = root.0.join("accounts").join("claude-main");
        fs::create_dir_all(&accounts).unwrap();
        insert_account(&db, "claude-main", "claude", &accounts);
        make_default_account(&db, "claude-main");
        let bots: Vec<StoredBot> = ["Developer A", "Developer B", "Reviewer", "Tester"]
            .iter()
            .map(|name| {
                let input: BotInput = serde_json::from_value(json!({
                    "name": name, "provider": "claude", "model": "test", "color": "blue", "shape": "circle",
                    "instructions": "", "autonomy": "full",
                }))
                .unwrap();
                create_bot(&db, &root.0.join("bots"), &input).unwrap()
            })
            .collect();
        let db: Db = Arc::new(Mutex::new(db));
        let worktrees = Arc::new(Worktrees::new(&managed).unwrap());
        let projects = Arc::new(Projects::new(db.clone(), worktrees.clone(), Arc::new(SqlRooms)));
        let runner = Arc::new(FakeRunner {
            db: db.clone(),
            delivered: Mutex::default(),
            active: Mutex::default(),
            tasks: OnceLock::new(),
            hang_retired: AtomicBool::new(false),
        });
        let tasks = Tasks::new(projects.clone(), runner.clone(), options);
        runner.tasks.set(Arc::downgrade(&tasks)).ok();
        Fixture {
            root,
            repo_path,
            base: String::new(),
            db,
            bots,
            worktrees,
            projects,
            repository: Repository {
                id: String::new(),
                path: String::new(),
                common_dir: String::new(),
                name: String::new(),
                created_at: String::new(),
            },
            project: serde_json::from_value(json!({
                "name": "", "repositoryId": "", "baseRef": "refs/heads/main", "targetRef": "refs/heads/main",
                "roomId": null, "members": [], "checks": [], "id": "", "state": "active", "revision": 1,
                "createdAt": "", "updatedAt": "",
            }))
            .unwrap(),
            tasks,
            runner,
            git_env,
        }
    }

    /// The repository, the base commit and the project, as `projectFixture` makes them.
    fn seed(&mut self) {
        let repo = self.repo_path.clone();
        self.git(&repo, &["init", "-b", "main"]);
        self.git(&repo, &["config", "user.name", "Synthetic fixture"]);
        self.git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        self.git(&repo, &["config", "commit.gpgsign", "false"]);
        fs::create_dir(repo.join("src")).unwrap();
        fs::write(repo.join("src").join("a.txt"), "A0\n").unwrap();
        fs::write(repo.join("src").join("b.txt"), "B0\n").unwrap();
        fs::write(repo.join(".gitignore"), "check-output.txt\n").unwrap();
        self.git(&repo, &["add", "."]);
        self.git(&repo, &["commit", "-m", "Synthetic base"]);
        self.base = self.git(&repo, &["rev-parse", "HEAD"]);
        let registered = {
            let db = self.db.lock().unwrap();
            self.projects.register(&db, repo.to_str().unwrap()).unwrap()
        };
        let comspec = std::env::var("ComSpec").expect("ComSpec");
        let input: ProjectInput = serde_json::from_value(json!({
            "name": "Synthetic project", "repositoryId": registered.id,
            "baseRef": "refs/heads/main", "targetRef": "refs/heads/main", "roomId": null,
            "members": self.bots.iter().enumerate().map(|(index, bot)| json!({
                "botId": bot.id,
                "roles": if index < 2 { ["developer"] } else if index == 2 { ["reviewer"] } else { ["tester"] },
                "instructions": "",
            })).collect::<Vec<_>>(),
            "checks": [{
                "id": "check", "label": "Synthetic accepted check", "command": comspec,
                "args": ["/d", "/c", "echo passed>check-output.txt"], "timeoutMs": 20000,
            }],
        }))
        .unwrap();
        let db = self.db.lock().unwrap();
        self.project = self.projects.create(&db, input).unwrap();
        self.repository = self.projects.repository(&db, &registered.id).unwrap();
    }

    /// Runs git in `path`, which must be inside the fixture's root, and returns its trimmed output.
    pub fn git(&self, path: &Path, args: &[&str]) -> String {
        assert!(
            daemon::fsid::is_within(path, &self.root.0).unwrap(),
            "{} is outside the fixture",
            path.display()
        );
        let mut command = spawn::command("git");
        command.args(args).current_dir(path).env_clear().envs(self.git_env.clone());
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    pub fn owner(&self) -> Owner {
        Owner {
            head: self.git(&self.repo_path, &["rev-parse", "HEAD"]),
            status: self.git(&self.repo_path, &["status", "--porcelain=v1"]),
            index: fs::read(self.repo_path.join(".git").join("index")).unwrap(),
        }
    }

    /// `newTask(index)`: a task for developer `index`, with the reviewer and tester assigned.
    pub fn new_task(&self, index: usize) -> ProjectTask {
        let input: TaskInput = serde_json::from_value(json!({
            "projectId": self.project.id, "title": format!("Task {index}"),
            "instructions": "Change the assigned file.", "acceptance": ["Check the change"],
            "assigneeId": self.bots[index].id, "reviewerId": self.bots[2].id, "testerId": self.bots[3].id,
        }))
        .unwrap();
        self.tasks.create(&self.db.lock().unwrap(), input).unwrap()
    }

    pub fn attempt(&self, id: &str) -> TaskAttempt {
        self.tasks.attempt(&self.db.lock().unwrap(), id).unwrap()
    }

    pub fn task(&self, id: &str) -> ProjectTask {
        self.tasks.task(&self.db.lock().unwrap(), id).unwrap()
    }

    pub fn work_item_count(&self) -> i64 {
        self.db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM work_items", [], |row| row.get(0))
            .unwrap()
    }

    /// `complete(attempt, file, bytes)`: the developer commits `bytes` to `file` and the turn ends successfully.
    pub async fn complete(&self, attempt: &TaskAttempt, file: &str, bytes: &str) -> TaskAttempt {
        let work = PathBuf::from(&attempt.worktree_path);
        fs::write(work.join(file), bytes).unwrap();
        self.git(&work, &["add", "--", file]);
        self.git(&work, &["commit", "-m", "Synthetic task commit"]);
        let item = get_work_item(&self.db.lock().unwrap(), attempt.work_item_id.as_deref().unwrap()).unwrap();
        self.runner.end_turn(&item, WorkStatus::Done, TurnOutcome::Done);
        let id = attempt.id.clone();
        eventually("the attempt to settle", || {
            let current = self.attempt(&id);
            (!matches!(current.state, AttemptState::Queued | AttemptState::Running)).then_some(current)
        })
        .await
    }
}
