//! `models.list` (`listModels` in `setup.ts`).

use protocol::ErrorCode;
use protocol::model::{ModelInfo, Provider};
use protocol::wire::method;

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

pub fn register(handlers: Handlers) -> Handlers {
    handlers.on::<method::ModelsList, _>(|params, _| async move {
        match params.provider {
            Provider::Claude => Ok(claude_models()),
            // T3.3 and T3.4: ask the Codex app server with the default account (setup.ts:470-482).
            Provider::Codex => Err(RpcError::new(
                ErrorCode::Unavailable,
                "Codex models aren't available yet.",
            )),
        }
    })
}
