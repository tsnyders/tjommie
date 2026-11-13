//! `task-execution.ts`: the identity of one task turn's execution context. `TaskExecutionContext` itself lives in
//! `runner.rs` (Core's). `standaloneExecutionKey` is Core's private function there, because only the runner uses it.

use std::path::Path;

use protocol::ErrorCode;
use sha2::{Digest, Sha256};

use crate::fsid::real_path;
use crate::runner::ExecutionPhase;
use crate::server::RpcError;

/// `"develop"`, `"review"`, `"test"` or `"discussion"`: what `JSON.stringify` stores for the phase.
pub fn phase_text(phase: ExecutionPhase) -> &'static str {
    match phase {
        ExecutionPhase::Develop => "develop",
        ExecutionPhase::Review => "review",
        ExecutionPhase::Test => "test",
        ExecutionPhase::Discussion => "discussion",
    }
}

/// `executionKey`: the SHA-256 TypeScript stores for `[botId, attemptId, revision, phase, realpath(cwd), instructions]`,
/// byte for byte, so a work item the old daemon queued still matches its context after an upgrade.
pub fn execution_key(
    bot_id: &str,
    attempt_id: &str,
    revision: u64,
    phase: ExecutionPhase,
    cwd: &str,
    instructions: &str,
) -> Result<String, RpcError> {
    let real = real_path(Path::new(cwd))
        .map_err(|error| RpcError::new(ErrorCode::BadState, error.to_string()))?;
    let json = serde_json::to_string(&(
        bot_id,
        attempt_id,
        revision,
        phase_text(phase),
        real.to_string_lossy(),
        instructions,
    ))
    .map_err(|error| RpcError::new(ErrorCode::Internal, error.to_string()))?;
    Ok(Sha256::digest(json)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
