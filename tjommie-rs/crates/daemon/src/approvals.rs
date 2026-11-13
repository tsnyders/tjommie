//! `approvals.ts`: a CLI's tool requests and a bot's routine proposals, waiting for your answer.

use protocol::ErrorCode;
use protocol::model::{Approval, ApprovalDecision, ApprovalKind, ApprovalStatus};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Map, Value};

use crate::bots::{from_text, text};
use crate::ids::now_iso;
use crate::server::RpcError;

#[derive(Debug, Clone, PartialEq)]
pub struct NewApproval {
    pub id: String,
    pub bot_id: String,
    pub kind: ApprovalKind,
    pub tool: String,
    pub summary: String,
    pub detail: String,
    pub rule: Option<String>,
    pub payload: Option<Map<String, Value>>,
    pub message_id: String,
}

const COLUMNS: &str =
    "id, bot_id, kind, tool, summary, detail, rule, payload, status, message_id, created_at";

fn read_approval(row: &Row) -> rusqlite::Result<Approval> {
    let payload: Option<String> = row.get(7)?;
    let payload = payload
        .map(|payload| serde_json::from_str(&payload))
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    Ok(Approval {
        id: row.get(0)?,
        bot_id: row.get(1)?,
        kind: from_text(row, 2)?,
        tool: row.get(3)?,
        summary: row.get(4)?,
        detail: row.get(5)?,
        rule: row.get(6)?,
        payload,
        status: from_text(row, 8)?,
        message_id: row.get(9)?,
        created_at: row.get(10)?,
    })
}

pub fn create_approval(db: &Connection, approval: &NewApproval) -> Result<Approval, RpcError> {
    db.execute(
        "INSERT INTO approvals (id, bot_id, kind, tool, summary, detail, rule, payload, status, message_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10)",
        params![
            approval.id,
            approval.bot_id,
            text(&approval.kind),
            approval.tool,
            approval.summary,
            approval.detail,
            approval.rule,
            approval.payload.as_ref().map(|payload| Value::Object(payload.clone()).to_string()),
            approval.message_id,
            now_iso(),
        ],
    )?;
    get_approval(db, &approval.id)
}

pub fn get_approval(db: &Connection, id: &str) -> Result<Approval, RpcError> {
    db.query_row(
        &format!("SELECT {COLUMNS} FROM approvals WHERE id = ?1"),
        [id],
        read_approval,
    )
    .optional()?
    .ok_or_else(|| RpcError::new(ErrorCode::NotFound, "No such approval"))
}

pub fn resolve_approval(
    db: &Connection,
    id: &str,
    decision: ApprovalDecision,
) -> Result<Approval, RpcError> {
    let approval = get_approval(db, id)?;
    if approval.status != ApprovalStatus::Pending {
        return Err(RpcError::new(
            ErrorCode::AlreadyAnswered,
            "This request was already answered",
        ));
    }
    let status = if decision == ApprovalDecision::Deny {
        ApprovalStatus::Denied
    } else {
        ApprovalStatus::Allowed
    };
    db.execute(
        "UPDATE approvals SET status = ?1, decided_at = ?2 WHERE id = ?3",
        [&text(&status), &now_iso(), id],
    )?;
    if let (ApprovalDecision::Always, Some(rule)) = (
        decision,
        approval.rule.as_deref().filter(|rule| !rule.is_empty()),
    ) {
        db.execute(
            "INSERT OR IGNORE INTO allow_rules (bot_id, rule) VALUES (?1, ?2)",
            [&approval.bot_id, rule],
        )?;
    }
    get_approval(db, id)
}

pub fn allow_rules(db: &Connection, bot_id: &str) -> Result<Vec<String>, RpcError> {
    Ok(db
        .prepare("SELECT rule FROM allow_rules WHERE bot_id = ?1 ORDER BY rule")?
        .query_map([bot_id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// After a restart the CLI that asked is gone, so nothing can answer its requests any more. Routine proposals don't
/// depend on a CLI, so they stay pending.
pub fn expire_pending_approvals(db: &Connection) -> Result<Vec<Approval>, RpcError> {
    let ids = db
        .prepare("SELECT id FROM approvals WHERE status = 'pending' AND kind = 'tool' ORDER BY created_at, rowid")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    db.execute("UPDATE approvals SET status = 'denied', decided_at = ?1 WHERE status = 'pending' AND kind = 'tool'", [now_iso()])?;
    ids.iter().map(|id| get_approval(db, id)).collect()
}
