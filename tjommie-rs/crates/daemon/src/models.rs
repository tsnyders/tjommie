//! Claude's model list for `models.list`. Codex asks its app server (`Setup::list_models`).

use protocol::model::ModelInfo;

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
