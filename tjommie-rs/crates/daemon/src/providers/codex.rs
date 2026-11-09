//! Drives `codex app-server`: one process and one thread per bot session.

use protocol::model::Autonomy;
use serde_json::{Map, Value, json};

use super::{McpServer, StartOptions};

/// `{ approvalPolicy, sandbox }` for a thread.
pub fn codex_policy(autonomy: Autonomy) -> Value {
    let (approval_policy, sandbox) = match autonomy {
        Autonomy::Ask => ("untrusted", "read-only"),
        Autonomy::Folder => ("on-request", "workspace-write"),
        Autonomy::Full => ("never", "danger-full-access"),
    };
    json!({ "approvalPolicy": approval_policy, "sandbox": sandbox })
}

/// Codex features that are on by default and would reach past Tjommie: ChatGPT apps, driving the person's own
/// browser and desktop, plugins, and memories shared by every bot on the account. Bots get Tjommie's MCP servers only.
pub const CODEX_FEATURES_OFF: [&str; 10] = [
    "apps",
    "browser_use",
    "browser_use_external",
    "computer_use",
    "in_app_browser",
    "plugins",
    "remote_plugin",
    "memories",
    "tool_suggest",
    "skill_mcp_dependency_install",
];

/// Per-thread config overrides. The daemon gates Own folder before this session starts. `windows`: the platform is
/// Windows (`platform === "win32"` in TypeScript).
pub fn codex_config(options: &StartOptions, windows: bool) -> Value {
    let elevated = options.autonomy == Autonomy::Folder || (options.autonomy == Autonomy::Ask && options.codex_sandbox_ready);
    let mut config = Map::new();
    config.insert("sandbox_workspace_write".into(), json!({ "network_access": false, "writable_roots": options.extra_dirs }));
    if windows {
        config.insert("windows".into(), json!({ "sandbox": if elevated { "elevated" } else { "unelevated" } }));
    }
    config.insert("features".into(), CODEX_FEATURES_OFF.iter().map(|feature| (feature.to_string(), json!(false))).collect::<Map<_, _>>().into());
    let servers = options.mcp_servers.iter().map(|server| {
        let config = match server {
            McpServer::Http { url, headers, .. } => json!({ "url": url, "http_headers": headers }),
            McpServer::Stdio { command, args, env, .. } => json!({ "command": command, "args": args, "env": env }),
        };
        (server.name().to_string(), config)
    });
    config.insert("mcp_servers".into(), servers.collect::<Map<_, _>>().into());
    config.into()
}
