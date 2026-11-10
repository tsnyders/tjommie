//! `models.list` (`listModels` in `setup.ts`).

use std::collections::BTreeMap;
use std::sync::Arc;

use protocol::ErrorCode;
use protocol::model::{ModelInfo, Provider};
use protocol::wire::method;
use serde::Deserialize;
use serde_json::json;

use crate::cli::CliHost;
use crate::context::Context;
use crate::runner::{ResolveCli, default_account_env};
use crate::server::{Handlers, RpcError};

/// `CLAUDE_MODELS`: Claude Code takes these aliases and resolves the latest model behind each.
pub fn claude_models() -> Vec<ModelInfo> {
    [
        ("sonnet", "Claude Sonnet (latest)", true),
        ("opus", "Claude Opus (latest)", false),
        ("haiku", "Claude Haiku (latest)", false),
    ]
    .map(|(id, label, is_default)| ModelInfo {
        id: id.to_owned(),
        label: label.to_owned(),
        is_default,
    })
    .to_vec()
}

#[derive(Deserialize)]
struct ModelList {
    data: Vec<CodexModel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexModel {
    model: String,
    display_name: String,
    hidden: bool,
    is_default: bool,
}

fn internal(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::Internal, message)
}

/// Codex's half of `listModels`: an app-server on the default account (`env`, in its `folder`), asked `model/list`
/// and always closed afterwards. `cmd` is `resolveCli("codex")`.
pub async fn list_codex_models(
    host: &dyn CliHost,
    cmd: Option<crate::cli::CliCommand>,
    env: &BTreeMap<String, String>,
    folder: &str,
) -> Result<Vec<ModelInfo>, RpcError> {
    let Some(cmd) = cmd else {
        return Err(RpcError::new(
            ErrorCode::NotInstalled,
            "Codex is not installed",
        ));
    };
    let server = host
        .start_codex_server(&cmd, env, folder, false)
        .await
        .map_err(|error| internal(error.message()))?;
    let listed = server.rpc.request("model/list", json!({})).await;
    (server.close)();
    server.exit.wait().await;
    let listed = listed.map_err(|error| internal(error.message()))?;
    let ModelList { data } = serde_json::from_value(listed)
        .map_err(|_| internal("Codex returned an invalid model list."))?;
    Ok(data
        .into_iter()
        .filter(|model| !model.hidden)
        .map(|model| ModelInfo {
            id: model.model,
            label: model.display_name,
            is_default: model.is_default,
        })
        .collect())
}

/// Where Codex's CLI comes from: T3.4's real host and `resolveCli`. Without one, Codex answers unavailable.
#[derive(Clone)]
pub struct CodexCli {
    pub host: Arc<dyn CliHost>,
    pub resolve: ResolveCli,
}

pub fn register(handlers: Handlers, context: &Context, codex: Option<CodexCli>) -> Handlers {
    let context = context.clone();
    handlers.on::<method::ModelsList, _>(move |params, _| {
        // `resolveCli` first, then the default account, as `listModels` does.
        let codex = match (params.provider, &codex) {
            (Provider::Codex, Some(cli)) => {
                let cmd = (cli.resolve)(Provider::Codex);
                let account = cmd
                    .as_ref()
                    .map(|_| context.with_db(|db| default_account_env(db, Provider::Codex)));
                Some((cli.host.clone(), cmd, account))
            }
            _ => None,
        };
        async move {
            match (params.provider, codex) {
                (Provider::Claude, _) => Ok(claude_models()),
                (Provider::Codex, Some((host, cmd, account))) => {
                    let (env, folder) = account.transpose()?.unwrap_or_default();
                    list_codex_models(host.as_ref(), cmd, &env, &folder).await
                }
                (Provider::Codex, None) => Err(RpcError::new(
                    ErrorCode::Unavailable,
                    "Codex models aren't available yet.",
                )),
            }
        }
    })
}
