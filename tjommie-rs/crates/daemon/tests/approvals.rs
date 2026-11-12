//! `approvals.test.ts`

mod common;

use std::cell::Cell;

use common::{TempRoot, new_bot, test_db};
use daemon::approvals::{
    NewApproval, allow_rules, create_approval, expire_pending_approvals, get_approval,
    resolve_approval,
};
use daemon::chat::{NewMessage, add_message};
use protocol::ErrorCode;
use protocol::model::{
    Approval, ApprovalDecision, ApprovalKind, ApprovalStatus, MessageKind, MessageStatus,
};
use rusqlite::Connection;
use serde_json::{Value, json};

struct Fixture {
    db: Connection,
    bot_id: String,
    n: Cell<u32>,
    _root: TempRoot,
}

fn setup() -> Fixture {
    let root = TempRoot::new();
    let db = test_db(&root.0);
    let bot_id = new_bot(&db, &root.0.join("bots"), "Writer", "claude").id;
    Fixture {
        db,
        bot_id,
        n: Cell::new(0),
        _root: root,
    }
}

impl Fixture {
    fn message(&self, body: &str) -> String {
        let new = NewMessage {
            conversation_id: self.bot_id.clone(),
            author: self.bot_id.clone(),
            kind: MessageKind::Approval,
            body: body.into(),
            data: None,
            status: MessageStatus::Done,
        };
        add_message(&self.db, &new).unwrap().id
    }

    fn approval(&self) -> Approval {
        self.n.set(self.n.get() + 1);
        create_approval(&self.db, &self.new_approval(&format!("a{}", self.n.get()))).unwrap()
    }

    fn new_approval(&self, id: &str) -> NewApproval {
        NewApproval {
            id: id.into(),
            bot_id: self.bot_id.clone(),
            kind: ApprovalKind::Tool,
            tool: "Write".into(),
            summary: "hello.txt".into(),
            detail: "{}".into(),
            rule: Some("Write".into()),
            payload: None,
            message_id: self.message("hello.txt"),
        }
    }
}

#[test]
fn creates_a_pending_approval() {
    let f = setup();
    let approval = f.approval();
    assert_eq!(
        (
            approval.bot_id.as_str(),
            approval.tool.as_str(),
            approval.summary.as_str(),
            approval.rule.as_deref(),
            approval.status
        ),
        (
            f.bot_id.as_str(),
            "Write",
            "hello.txt",
            Some("Write"),
            ApprovalStatus::Pending
        )
    );
}

#[test]
fn keeps_routine_proposals_with_their_payload_pending_after_a_restart_other_approvals_are_kind_tool()
 {
    let f = setup();
    let Value::Object(payload) = json!({ "name": "Morning check" }) else {
        unreachable!()
    };
    let new = NewApproval {
        kind: ApprovalKind::Routine,
        payload: Some(payload.clone()),
        tool: "Routine".into(),
        summary: "s".into(),
        detail: "d".into(),
        rule: None,
        message_id: f.message("Add a routine"),
        ..f.new_approval("r1")
    };
    let routine = create_approval(&f.db, &new).unwrap();
    assert_eq!(
        (routine.kind, routine.payload),
        (ApprovalKind::Routine, Some(payload))
    );
    let tool = f.approval();
    assert_eq!((tool.kind, tool.payload), (ApprovalKind::Tool, None));
    expire_pending_approvals(&f.db).unwrap();
    assert_eq!(
        get_approval(&f.db, "r1").unwrap().status,
        ApprovalStatus::Pending
    );
}

#[test]
fn allows_once_without_saving_the_rule() {
    let f = setup();
    let approval = f.approval();
    assert_eq!(
        resolve_approval(&f.db, &approval.id, ApprovalDecision::Once)
            .unwrap()
            .status,
        ApprovalStatus::Allowed
    );
    assert_eq!(allow_rules(&f.db, &f.bot_id).unwrap(), Vec::<String>::new());
}

#[test]
fn saves_the_rule_on_always_allow_once_per_rule() {
    let f = setup();
    resolve_approval(&f.db, &f.approval().id, ApprovalDecision::Always).unwrap();
    resolve_approval(&f.db, &f.approval().id, ApprovalDecision::Always).unwrap();
    assert_eq!(allow_rules(&f.db, &f.bot_id).unwrap(), ["Write"]);
}

#[test]
fn denies_and_refuses_to_answer_the_same_approval_twice() {
    let f = setup();
    let approval = f.approval();
    assert_eq!(
        resolve_approval(&f.db, &approval.id, ApprovalDecision::Deny)
            .unwrap()
            .status,
        ApprovalStatus::Denied
    );
    let error = resolve_approval(&f.db, &approval.id, ApprovalDecision::Once).unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (
            ErrorCode::AlreadyAnswered,
            "This request was already answered"
        )
    );
}

#[test]
fn closes_every_pending_approval_after_a_restart() {
    let f = setup();
    let pending = f.approval();
    let answered = f.approval();
    resolve_approval(&f.db, &answered.id, ApprovalDecision::Once).unwrap();
    let expired: Vec<String> = expire_pending_approvals(&f.db)
        .unwrap()
        .into_iter()
        .map(|approval| approval.id)
        .collect();
    assert_eq!(expired, std::slice::from_ref(&pending.id));
    assert_eq!(
        get_approval(&f.db, &pending.id).unwrap().status,
        ApprovalStatus::Denied
    );
    let error = get_approval(&f.db, "missing").unwrap_err();
    assert_eq!(
        (error.code, error.message.as_str()),
        (ErrorCode::NotFound, "No such approval")
    );
}
