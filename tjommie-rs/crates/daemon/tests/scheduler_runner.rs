mod m4_runner_support;
use m4_runner_support::*;

use daemon::queue::{WorkStatus, set_status};
use daemon::routines::{create_routine, list_runs, parse_iso};
use protocol::model::{MessageData, RoutineInput, RoutineRunStatus, message_data};
use serde_json::json;

/// `at(iso)` in the TypeScript test: the instant as milliseconds.
fn at(iso: &str) -> i64 {
    parse_iso(iso).expect("a valid ISO time")
}

// `scheduler.test.ts`: the one case that builds the real Runner. The scheduler is M4's, so its `deliver` goes through the
// runner and its turn hooks are M4's; the TypeScript test wired its own. Its helpers (`at`, `digest`, the scheduler
// factory) map to `at` here, the routine input below, and `h.m4.scheduler`.
#[tokio::test(flavor = "multi_thread")]
async fn hands_persisted_work_to_the_real_runner_once_and_integrates_interrupted_run_recovery() {
    // Signed out, so the queue holds: nothing touches accounts or launches a CLI. The TypeScript `createSession` throws;
    // here no session may be made, which is checked at the end.
    let h = harness(false);
    let bot = h.create(json!({
        "name": "Night Shift", "color": "purple", "shape": "cloud", "instructions": "",
        "provider": "codex", "model": "gpt-5.6-sol",
    }));
    let input: RoutineInput = serde_json::from_value(json!({
        "botId": bot.id, "name": "Digest", "prompt": "Summarize my inbox",
        "days": [0, 1, 2, 3, 4, 5, 6], "times": ["08:00", "17:00"], "timeZone": "UTC",
    }))
    .unwrap();
    let routine = create_routine(&h.conn(), &input, at("2026-09-20T00:00:00Z")).unwrap();

    h.m4.scheduler.tick(at("2026-09-20T08:00:10Z")).unwrap();
    assert_eq!(h.all_work().len(), 1);
    // `delivered[0]`: the one work row the timer queued and handed to the runner.
    let item = h.latest_work();
    h.m4.scheduler.run_started(&h.conn(), &item).unwrap();
    set_status(&h.conn(), &item.id, WorkStatus::Running).unwrap();
    h.runner.recover().unwrap();
    h.m4.scheduler.recover().unwrap();

    assert_eq!(h.status(&item.id), WorkStatus::Interrupted);
    let runs = list_runs(&h.conn(), &routine.id).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RoutineRunStatus::Failed);
    assert_eq!(
        runs[0].note.as_deref(),
        Some("Tjommie restarted while this was running.")
    );
    let label_statuses: Vec<RoutineRunStatus> = h
        .chat(&bot.id, 10)
        .iter()
        .filter_map(|message| match message_data(message) {
            Ok(MessageData::Routine(label)) => Some(label.status),
            _ => None,
        })
        .collect();
    assert_eq!(label_statuses, vec![RoutineRunStatus::Failed]);
    assert_eq!(h.sessions_made(), 0, "no CLI session was started");
    h.runner.shutdown().await.unwrap();
}
