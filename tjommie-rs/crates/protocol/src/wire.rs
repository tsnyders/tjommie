//! `wire.ts`: frames, error codes, and every method and event.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{Message, Provider};
use crate::{
    Wire, accounts, attachments, browser, contract, diagnostics, google, model, projects, remote,
    vault,
};

pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadParams,
    NotFound,
    Busy,
    NameTaken,
    AlreadyAnswered,
    BadState,
    NotInstalled,
    NoAccount,
    UnknownMethod,
    Internal,
    Forbidden,
    Unavailable,
    Expired,
    Corrupt,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 14] = [
        ErrorCode::BadParams,
        ErrorCode::NotFound,
        ErrorCode::Busy,
        ErrorCode::NameTaken,
        ErrorCode::AlreadyAnswered,
        ErrorCode::BadState,
        ErrorCode::NotInstalled,
        ErrorCode::NoAccount,
        ErrorCode::UnknownMethod,
        ErrorCode::Internal,
        ErrorCode::Forbidden,
        ErrorCode::Unavailable,
        ErrorCode::Expired,
        ErrorCode::Corrupt,
    ];
}

/// `{}`. Whether extra keys are allowed is the method's schema's call; serde drops them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Empty {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Id {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderParams {
    pub provider: Provider,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BotsUpdate {
    pub id: String,
    pub patch: model::BotPatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotsPause {
    pub id: String,
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notes {
    pub text: String,
}

fn page_limit() -> u32 {
    50
}

fn team_room() -> String {
    model::TEAM_ROOM_ID.to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHistory {
    pub bot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default = "page_limit")]
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub messages: Vec<Message>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSend {
    pub bot_id: String,
    pub text: String,
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStop {
    pub bot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRetry {
    pub message_id: String,
    /// The bot whose failed room turn to retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalsRespond {
    pub approval_id: String,
    pub decision: model::ApprovalDecision,
}

/// Older callers leave out the room: it defaults to the Team room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomHistory {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default = "page_limit")]
    pub limit: u32,
    #[serde(default = "team_room")]
    pub room_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomPost {
    #[serde(default = "team_room")]
    pub room_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomsUpdate {
    pub id: String,
    pub patch: model::RoomPatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeView {
    pub messages: Vec<Message>,
    pub has_more: bool,
    pub exchange: model::Exchange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutinesList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutinesUpdate {
    pub id: String,
    pub patch: model::RoutinePatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutinesRuns {
    pub routine_id: String,
}

/// A durable notification, kept until acknowledged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PendingNotification {
    RoutineFailed {
        bot_id: String,
        routine_id: String,
        run_id: String,
        title: String,
        body: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationsAck {
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDelta {
    pub id: String,
    pub conversation_id: String,
    pub delta: String,
}

/// A provider's sign-in ended, sent after the fresh `setup.changed`. Also sent when it changed nothing: the window was
/// closed, the time ran out or the login failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupSignInEnded {
    pub provider: Provider,
    pub attempt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Notify {
    Approval {
        bot_id: String,
        title: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_id: Option<String>,
    },
    Limit {
        bot_id: String,
        title: String,
        body: String,
    },
    RoutineFailed {
        bot_id: String,
        title: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        routine_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
    },
    Done {
        bot_id: String,
        title: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        room_id: Option<String>,
    },
    Failed {
        bot_id: String,
        title: String,
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        room_id: Option<String>,
    },
}

macro_rules! methods {
    ($($name:literal => $variant:ident($params:ty) -> $result:ty,)*) => {
        /// Every method the daemon answers, in `wire.ts` order.
        pub const METHODS: &[&str] = &[$($name),*];

        /// A request's method and params. Build one with [`Request::decode`].
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(tag = "method", content = "params")]
        pub enum Request {
            $(#[serde(rename = $name)] $variant($params),)*
        }

        /// One marker type per method, named like its [`Request`] variant, for typed handlers.
        pub mod method {
            $(pub struct $variant;)*
        }

        $(
            impl Method for method::$variant {
                const NAME: &'static str = $name;
                type Params = $params;
                type Result = $result;

                fn take(request: Request) -> Option<$params> {
                    match request {
                        Request::$variant(params) => Some(params),
                        _ => None,
                    }
                }
            }
        )*
    };
}

/// A method: its wire name, its params and what it answers.
pub trait Method {
    const NAME: &'static str;
    type Params: Send + 'static;
    type Result: Serialize + Send + 'static;

    /// The params, if `request` is this method's.
    fn take(request: Request) -> Option<Self::Params>;
}

methods! {
    "vault.status" => VaultStatus(Empty) -> vault::VaultStatus,
    "vault.list" => VaultList(vault::VaultList) -> Vec<vault::LoginEntry>,
    "vault.unlock" => VaultUnlock(Empty) -> vault::VaultStatus,
    "vault.lock" => VaultLock(Empty) -> vault::VaultStatus,
    "vault.cancel" => VaultCancel(Empty) -> Empty,
    "vault.entry" => VaultEntry(vault::VaultEntry) -> vault::VaultEntryResult,
    "vault.delete" => VaultDelete(vault::EntryVersion) -> vault::VaultDeleteResult,
    "vault.fill" => VaultFill(vault::VaultFill) -> vault::VaultFillResult,
    "vault.finish" => VaultFinish(vault::VaultFinish) -> vault::VaultFinishResult,
    "diagnostics.get" => DiagnosticsGet(Empty) -> diagnostics::HostDiagnostics,
    "attachments.metadata" => AttachmentsMetadata(attachments::AttachmentRef) -> attachments::AttachmentInfo,
    "attachments.open" => AttachmentsOpen(attachments::AttachmentRef) -> attachments::AttachmentOpen,
    "attachments.read" => AttachmentsRead(attachments::AttachmentRead) -> attachments::AttachmentChunk,
    "attachments.close" => AttachmentsClose(attachments::TransferId) -> Empty,
    "projects.repositories" => ProjectsRepositories(Empty) -> Vec<projects::ProjectRepository>,
    "projects.registerRepository" => ProjectsRegisterRepository(projects::RepositoryPath) -> projects::ProjectRepository,
    "projects.list" => ProjectsList(Empty) -> Vec<projects::Project>,
    "projects.get" => ProjectsGet(Id) -> projects::Project,
    "projects.heads" => ProjectsHeads(Id) -> projects::ProjectHeads,
    "projects.create" => ProjectsCreate(projects::ProjectInput) -> projects::Project,
    "projects.update" => ProjectsUpdate(projects::ProjectsUpdate) -> projects::Project,
    "projects.archive" => ProjectsArchive(projects::ExpectedRevision) -> projects::Project,
    "tasks.list" => TasksList(projects::ProjectId) -> Vec<projects::ProjectTask>,
    "tasks.create" => TasksCreate(projects::TaskInput) -> projects::ProjectTask,
    "tasks.update" => TasksUpdate(projects::TasksUpdate) -> projects::ProjectTask,
    "tasks.get" => TasksGet(Id) -> projects::TaskDetail,
    "tasks.start" => TasksStart(projects::TaskRequest) -> projects::TaskAttempt,
    "tasks.resume" => TasksResume(projects::TaskRequest) -> projects::TaskAttempt,
    "tasks.stop" => TasksStop(projects::ExpectedRevision) -> projects::ProjectTask,
    "tasks.cancel" => TasksCancel(projects::ExpectedRevision) -> projects::ProjectTask,
    "tasks.diff" => TasksDiff(projects::AttemptId) -> projects::TaskDiff,
    "tasks.cleanupPreview" => TasksCleanupPreview(projects::AttemptId) -> projects::TaskCleanupPreview,
    "tasks.cleanup" => TasksCleanup(projects::TasksCleanup) -> projects::TaskAttempt,
    "tasks.evaluate" => TasksEvaluate(projects::TasksEvaluate) -> projects::TaskEvaluation,
    "tasks.integrate" => TasksIntegrate(projects::TasksIntegrate) -> projects::TaskIntegration,
    "tasks.promote" => TasksPromote(projects::TasksPromote) -> projects::TaskIntegration,
    "remote.status" => RemoteStatus(Empty) -> remote::RemoteStatus,
    "remote.ping" => RemotePing(Empty) -> Empty,
    "devices.list" => DevicesList(Empty) -> Vec<remote::Device>,
    "devices.rename" => DevicesRename(remote::DevicesRename) -> remote::Device,
    "devices.revoke" => DevicesRevoke(Id) -> remote::Device,
    "device.registerPush" => DeviceRegisterPush(remote::DeviceRegisterPush) -> Empty,
    "pairing.create" => PairingCreate(Empty) -> remote::PairingStart,
    "pairing.cancel" => PairingCancel(Id) -> Empty,
    "accounts.list" => AccountsList(Empty) -> Vec<accounts::AccountInfo>,
    "accounts.create" => AccountsCreate(accounts::AccountInput) -> accounts::AccountInfo,
    "accounts.update" => AccountsUpdate(accounts::AccountsUpdate) -> accounts::AccountInfo,
    "accounts.remove" => AccountsRemove(Id) -> Empty,
    "accounts.check" => AccountsCheck(accounts::AccountsCheck) -> Vec<accounts::AccountInfo>,
    "accounts.retryLimit" => AccountsRetryLimit(Id) -> accounts::AccountInfo,
    "accounts.signIn" => AccountsSignIn(Id) -> accounts::SignInStart,
    "accounts.cancelSignIn" => AccountsCancelSignIn(accounts::AccountAttempt) -> Empty,
    "accounts.prepareCodexSandbox" => AccountsPrepareCodexSandbox(Id) -> accounts::AccountInfo,
    "usage.query" => UsageQuery(accounts::UsageQuery) -> accounts::UsageReport,
    "browser.open" => BrowserOpen(browser::BotTarget) -> browser::BrowserState,
    "browser.frame" => BrowserFrame(browser::BrowserFrameParams) -> browser::BrowserFrameResult,
    "browser.control" => BrowserControl(browser::BrowserControlParams) -> browser::BrowserState,
    "browser.navigate" => BrowserNavigate(browser::BrowserNavigate) -> browser::BrowserState,
    "browser.history" => BrowserHistory(browser::BrowserHistory) -> browser::BrowserState,
    "browser.tab" => BrowserTab(browser::BrowserTabParams) -> browser::BrowserState,
    "browser.resize" => BrowserResize(browser::BrowserResize) -> browser::BrowserState,
    "browser.input" => BrowserInput(browser::BrowserInputParams) -> Empty,
    "browser.dialog" => BrowserDialog(browser::BrowserDialogParams) -> browser::BrowserState,
    "google.status" => GoogleStatus(Empty) -> google::GoogleStatus,
    "google.configure" => GoogleConfigure(google::GoogleClientConfig) -> google::GoogleStatus,
    "google.begin" => GoogleBegin(google::GoogleBegin) -> google::GoogleAuthorization,
    "google.cancel" => GoogleCancel(google::GoogleCancel) -> google::GoogleStatus,
    "google.disconnect" => GoogleDisconnect(Empty) -> google::GoogleStatus,
    "google.grant" => GoogleGrant(google::GoogleGrant) -> google::GoogleStatus,
    "setup.status" => SetupStatus(Empty) -> Vec<model::ProviderSetup>,
    "setup.signIn" => SetupSignIn(ProviderParams) -> accounts::SignInStart,
    "setup.prepareCodexSandbox" => SetupPrepareCodexSandbox(Empty) -> model::ProviderSetup,
    "models.list" => ModelsList(ProviderParams) -> Vec<model::ModelInfo>,
    "bots.list" => BotsList(Empty) -> Vec<model::Bot>,
    "bots.create" => BotsCreate(model::BotInput) -> model::Bot,
    "bots.update" => BotsUpdate(BotsUpdate) -> model::Bot,
    "bots.delete" => BotsDelete(Id) -> Empty,
    "bots.pause" => BotsPause(BotsPause) -> model::Bot,
    "bots.showBrowser" => BotsShowBrowser(Id) -> Empty,
    "bots.notes" => BotsNotes(Id) -> Notes,
    "chat.history" => ChatHistory(ChatHistory) -> MessagePage,
    "chat.send" => ChatSend(ChatSend) -> Message,
    "chat.stop" => ChatStop(ChatStop) -> Empty,
    "chat.retry" => ChatRetry(ChatRetry) -> Message,
    "approvals.respond" => ApprovalsRespond(ApprovalsRespond) -> model::Approval,
    "room.history" => RoomHistory(RoomHistory) -> MessagePage,
    "room.post" => RoomPost(RoomPost) -> Message,
    "rooms.list" => RoomsList(Empty) -> Vec<model::Room>,
    "rooms.create" => RoomsCreate(model::RoomInput) -> model::Room,
    "rooms.update" => RoomsUpdate(RoomsUpdate) -> model::Room,
    "rooms.delete" => RoomsDelete(Id) -> Empty,
    "exchanges.get" => ExchangesGet(Id) -> ExchangeView,
    "routines.list" => RoutinesList(RoutinesList) -> Vec<model::Routine>,
    "routines.create" => RoutinesCreate(model::RoutineInput) -> model::Routine,
    "routines.update" => RoutinesUpdate(RoutinesUpdate) -> model::Routine,
    "routines.delete" => RoutinesDelete(Id) -> Empty,
    "routines.runs" => RoutinesRuns(RoutinesRuns) -> Vec<model::RoutineRun>,
    "notifications.pending" => NotificationsPending(Empty) -> Vec<PendingNotification>,
    "notifications.ack" => NotificationsAck(NotificationsAck) -> Empty,
    "settings.get" => SettingsGet(Empty) -> model::Settings,
    "settings.set" => SettingsSet(model::SettingsPatch) -> model::Settings,
}

macro_rules! events {
    ($($name:literal => $variant:ident($data:ty),)*) => {
        /// Everything the daemon pushes to connected clients, in `wire.ts` order.
        pub const EVENTS: &[&str] = &[$($name),*];

        /// An event and its data; serializes as `{ event, data }`.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(tag = "event", content = "data")]
        pub enum Event {
            $(#[serde(rename = $name)] $variant($data),)*
        }
    };
}

events! {
    "project.updated" => ProjectUpdated(projects::Project),
    "task.updated" => TaskUpdated(projects::ProjectTask),
    "task.attemptUpdated" => TaskAttemptUpdated(projects::TaskAttempt),
    "task.evaluationUpdated" => TaskEvaluationUpdated(projects::TaskEvaluation),
    "task.integrationUpdated" => TaskIntegrationUpdated(projects::TaskIntegration),
    "devices.changed" => DevicesChanged(Empty),
    "accounts.changed" => AccountsChanged(Vec<accounts::AccountInfo>),
    "accounts.signInEnded" => AccountsSignInEnded(accounts::AccountSignInEnded),
    "usage.changed" => UsageChanged(accounts::UsageChanged),
    "google.changed" => GoogleChanged(google::GoogleStatus),
    "message.added" => MessageAdded(Message),
    "message.updated" => MessageUpdated(Message),
    "message.delta" => MessageDelta(MessageDelta),
    "bot.updated" => BotUpdated(model::Bot),
    "bot.deleted" => BotDeleted(Id),
    "approval.added" => ApprovalAdded(model::Approval),
    "approval.resolved" => ApprovalResolved(model::Approval),
    "setup.changed" => SetupChanged(Vec<model::ProviderSetup>),
    "setup.signInEnded" => SetupSignInEnded(SetupSignInEnded),
    "notify" => Notify(Notify),
    "routine.updated" => RoutineUpdated(model::Routine),
    "routine.deleted" => RoutineDeleted(Id),
    "room.updated" => RoomUpdated(model::Room),
    "room.deleted" => RoomDeleted(Id),
    "routine.run" => RoutineRun(model::RoutineRun),
    "settings.changed" => SettingsChanged(model::Settings),
}

/// Why a request's method or params were refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnknownMethod,
    BadParams(String),
}

impl DecodeError {
    pub fn code(&self) -> ErrorCode {
        match self {
            DecodeError::UnknownMethod => ErrorCode::UnknownMethod,
            DecodeError::BadParams(_) => ErrorCode::BadParams,
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::UnknownMethod => formatter.write_str("Unknown method"),
            DecodeError::BadParams(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for DecodeError {}

impl Request {
    /// `methods[method].params.parse(params)`: trims and fills defaults as zod does, and checks the params schema.
    pub fn decode(method: &str, params: Value) -> Result<Self, DecodeError> {
        let params = contract::decode_params(method, params)?;
        serde_json::from_value(json!({ "method": method, "params": params }))
            .map_err(|error| DecodeError::BadParams(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientMessage {
    Hello {
        protocol: i64,
        token: String,
    },
    /// `params` stays raw until [`Request::decode`] checks it against the method. The key is required, even for methods
    /// without params: zod 4.6.5 rejects a missing `z.unknown()` key ("expected nonoptional"), so `server.ts` closes such
    /// a frame with 4400 rather than answering `bad_params`.
    Req {
        id: i64,
        method: String,
        params: Value,
    },
}

impl Wire for ClientMessage {
    const NAME: &'static str = "ClientMessage";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseError {
    pub code: ErrorCode,
    pub message: String,
}

/// A `res` frame: `{ id, ok: true, result }` or `{ id, ok: false, error }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: i64,
    pub ok: bool,
    /// The method's result, serialized from its [`Method::Result`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

impl Response {
    pub fn ok(id: i64, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: i64, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(ResponseError {
                code,
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[expect(
    clippy::large_enum_variant,
    reason = "frames are serialized as soon as they are built, never stored"
)]
pub enum ServerMessage {
    Welcome { protocol: u32 },
    Res(Response),
    Event(Event),
}
