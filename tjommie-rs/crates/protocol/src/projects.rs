//! `projects.ts`. `ProjectHandlers` and `ProjectBroadcast` have no port: they are TypeScript typing helpers.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectRole {
    Lead,
    Developer,
    Reviewer,
    Tester,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMember {
    pub bot_id: String,
    pub roles: Vec<ProjectRole>,
    #[serde(default)]
    pub instructions: String,
}

fn check_timeout_ms() -> u64 {
    120_000
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCheck {
    pub id: String,
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default = "check_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRepository {
    pub id: String,
    pub name: String,
    pub created_at: String,
}

/// `projects.create` params. Refs are `refs/heads/...`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInput {
    pub name: String,
    pub repository_id: String,
    pub base_ref: String,
    pub target_ref: String,
    #[serde(default)]
    pub room_id: Option<String>,
    pub members: Vec<ProjectMember>,
    #[serde(default)]
    pub checks: Vec<ProjectCheck>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectState {
    Active,
    Archived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    #[serde(flatten)]
    pub input: ProjectInput,
    pub id: String,
    pub state: ProjectState,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

/// Fields left out stay unchanged; create defaults don't apply.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub room_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<ProjectMember>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<Vec<ProjectCheck>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Todo,
    Running,
    Review,
    Done,
    Blocked,
    Cancelled,
}

/// `tasks.create` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInput {
    pub project_id: String,
    pub title: String,
    pub instructions: String,
    #[serde(default)]
    pub acceptance: Vec<String>,
    pub assignee_id: String,
    #[serde(default)]
    pub reviewer_id: Option<String>,
    #[serde(default)]
    pub tester_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTask {
    #[serde(flatten)]
    pub input: TaskInput,
    pub id: String,
    pub state: TaskState,
    pub current_attempt_id: Option<String>,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub reviewer_id: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub tester_id: Option<Option<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskPhase {
    Develop,
    Review,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttemptState {
    Provisioning,
    Queued,
    Running,
    Interrupted,
    Completed,
    Failed,
    Blocked,
    Cancelled,
    Cleaned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAttempt {
    pub id: String,
    pub task_id: String,
    pub ordinal: u64,
    pub base_sha: String,
    pub head_sha: Option<String>,
    pub branch: String,
    pub worktree_path: String,
    pub phase: TaskPhase,
    pub state: AttemptState,
    pub revision: u64,
    pub bot_id: String,
    pub work_item_id: Option<String>,
    pub context_key: String,
    pub reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFinding {
    pub severity: Severity,
    pub path: String,
    pub line: Option<u64>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoveredFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewCoverage {
    pub source_sha: String,
    pub snapshot_sha256: String,
    pub diff_sha256: String,
    /// Always true.
    pub complete: bool,
    pub files: Vec<CoveredFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRun {
    pub check_id: String,
    pub command: String,
    pub args: Vec<String>,
    pub exit_code: Option<i64>,
    pub output: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvaluationKind {
    Review,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvaluationState {
    Running,
    Passed,
    Findings,
    Failed,
    Unavailable,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvaluation {
    pub id: String,
    pub attempt_id: String,
    pub revision: u64,
    pub source_sha: String,
    pub project_revision: u64,
    pub kind: EvaluationKind,
    pub bot_id: Option<String>,
    pub state: EvaluationState,
    pub worktree_path: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub review_coverage: Option<Option<ReviewCoverage>>,
    pub reason: Option<String>,
    pub findings: Vec<TaskFinding>,
    pub checks: Vec<CheckRun>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IntegrationState {
    Preparing,
    Checking,
    Ready,
    Integrated,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskIntegration {
    pub id: String,
    pub attempt_id: String,
    pub revision: u64,
    pub accepted_sha: String,
    pub expected_target_sha: String,
    #[serde(default)]
    pub checks_completed: bool,
    pub project_revision: u64,
    pub target_ref: String,
    pub candidate_sha: Option<String>,
    pub branch: String,
    pub worktree_path: String,
    pub promotion_path: Option<String>,
    pub state: IntegrationState,
    pub reason: Option<String>,
    pub checks: Vec<CheckRun>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDetail {
    pub task: ProjectTask,
    pub attempts: Vec<TaskAttempt>,
    pub evaluations: Vec<TaskEvaluation>,
    pub integrations: Vec<TaskIntegration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDiff {
    pub attempt_id: String,
    pub revision: u64,
    pub source_sha: String,
    pub base_sha: String,
    pub text: String,
    pub truncated: bool,
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCleanupPreview {
    pub attempt_id: String,
    pub worktree_path: String,
    pub head_sha: Option<String>,
    pub files: Vec<String>,
    pub truncated: bool,
    pub can_remove: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryPath {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectHeads {
    pub base_sha: String,
    pub target_sha: String,
}

/// A project or task at the revision the caller last saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedRevision {
    pub id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectsUpdate {
    pub id: String,
    pub expected_revision: u64,
    pub patch: ProjectPatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectId {
    pub project_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksUpdate {
    pub id: String,
    pub expected_revision: u64,
    pub patch: TaskPatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRequest {
    pub id: String,
    pub expected_revision: u64,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptId {
    pub attempt_id: String,
}

/// The literal `"remove_clean_worktree"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupConfirm {
    RemoveCleanWorktree,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksCleanup {
    pub attempt_id: String,
    pub expected_head_sha: String,
    pub confirm: CleanupConfirm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksEvaluate {
    pub attempt_id: String,
    pub expected_revision: u64,
    pub source_sha: String,
    pub kind: EvaluationKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksIntegrate {
    pub attempt_id: String,
    pub expected_revision: u64,
    pub source_sha: String,
    pub expected_target_sha: String,
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksPromote {
    pub id: String,
    pub expected_target_sha: String,
}
