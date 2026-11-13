//! "account-bound runner" from `runner.test.ts`: the runner on a real `AccountRuntime`, with synthetic provider probes
//! and in-memory API keys. Events reach the runner through each session's channel, so the tests settle after emitting.

use std::ops::Deref;
use std::sync::OnceLock;

use super::*;
use common::{HealthyOps, MemoryKeys};
use daemon::account_runtime::{AccountRuntime, AccountRuntimeOptions, AccountSelection};
use daemon::accounts_store::{AccountInput, create_account, replace_account_limits};
use daemon::providers::ApiKey;
use protocol::accounts::AccountKind;
use protocol::model::{MessageStatus, Provider};

/// The harness with its runner replaced by one on an account runtime, and every session's start options kept.
struct Stack {
    h: Harness,
    runtime: AccountRuntime,
    captured: Recorded<StartOptions>,
}

impl Deref for Stack {
    type Target = Harness;

    fn deref(&self) -> &Harness {
        &self.h
    }
}

async fn stack() -> Stack {
    let mut h = harness();
    let captured: Recorded<StartOptions> = Arc::default();
    let (made, record) = (h.sessions.clone(), captured.clone());
    let create_session: SessionFactory =
        Arc::new(move |_bot: &StoredBot, options: StartOptions| {
            record.lock().unwrap().push(options);
            let session = FakeSession::new();
            made.lock().unwrap().push(session.clone());
            Ok(session as Arc<dyn ProviderSession>)
        });
    let slot: Arc<OnceLock<Runner>> = Arc::default();
    let to_runner = slot.clone();
    let runtime = AccountRuntime::new(AccountRuntimeOptions {
        db: h.db.clone(),
        root: h.root.0.clone(),
        accounts: h.root.0.join("accounts"),
        credentials: Arc::new(MemoryKeys::default()),
        ops: Arc::new(HealthyOps),
        // Deferred, as `Accounts::new` does: the runner calls the runtime with its own locks held.
        on_changed: Some(Arc::new(move |_runtime: &AccountRuntime, _id: &str| {
            if let Some(runner) = to_runner.get().cloned() {
                tokio::spawn(async move { runner.setup_changed() });
            }
        })),
        on_sign_in_ended: None,
        on_cap: None,
        now: Arc::new(daemon::account_quota::now_ms),
    });
    let runner = Runner::new(RunnerDeps {
        accounts: Some(Arc::new(runtime.clone())),
        ..h.deps(create_session)
    });
    let _ = slot.set(runner.clone());
    h.runner = runner;
    runtime.check_all().await.unwrap();
    Stack {
        h,
        runtime,
        captured,
    }
}

impl Stack {
    /// `api(id, fallback, cap)`: a Codex API-key account with a private key, checked.
    async fn api(&self, id: &str, fallback: bool, cap: f64) {
        let input = AccountInput {
            id: Some(id.into()),
            provider: Provider::Codex,
            label: id.into(),
            kind: AccountKind::ApiKey,
            folder: self
                .h
                .root
                .0
                .join("accounts")
                .join(id)
                .display()
                .to_string(),
            credential_ref: None,
            is_default: false,
            is_fallback: fallback,
            spending_cap_usd: Some(cap),
        };
        create_account(&self.h.db(), input).unwrap();
        self.runtime
            .set_api_key(id, "synthetic-private-key")
            .await
            .unwrap();
        self.runtime.check(id).await.unwrap();
    }

    fn captured(&self, index: usize) -> StartOptions {
        recorded(&self.captured)[index].clone()
    }

    fn ready(&self, bot_id: &str) -> bool {
        matches!(
            self.runtime.inspect(&self.h.stored(bot_id)),
            AccountSelection::Ready(_)
        )
    }
}

/// `usage` in the TypeScript tests: an unpriced Codex turn, which the runner estimates.
fn codex_usage(event_id: &str) -> ProviderEvent {
    ProviderEvent::Usage(Usage {
        model: "gpt-5.3-codex".into(),
        tokens: TokenUsage {
            input: 100.0,
            cached_input: 20.0,
            output: 5.0,
            reasoning: 1.0,
        },
        cost_usd: None,
        total_cost_usd: None,
        event_id: Some(event_id.into()),
    })
}

fn codex_bot(s: &Stack, extra: Value) -> StoredBot {
    let mut input = json!({ "provider": "codex", "model": "gpt-5.3-codex" });
    input
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    s.bot(input)
}

#[tokio::test]
async fn attributes_late_usage_to_the_captured_old_provider_and_account_after_the_bot_switches() {
    let s = stack().await;
    let bot = codex_bot(&s, json!({}));
    s.runner.submit(&bot.id, "First", &[]).unwrap();
    wait_for(|| s.sent(0) > 0).await;
    let first = s.session(0);
    first.emit(session_id("old-thread"));
    settle().await;
    s.db()
        .execute("UPDATE bots SET provider = 'claude', account_id = 'claude-main', model = 'haiku' WHERE id = ?1", [&bot.id])
        .unwrap();
    s.runner.bot_changed(&bot.id, true).unwrap();
    first.emit(codex_usage("one"));
    first.emit(turn_done(true, None));
    settle().await;
    s.runner.submit(&bot.id, "Second", &[]).unwrap();
    wait_for(|| s.sent(1) > 0).await;
    s.session(1).emit(session_id("new-thread"));
    first.emit(codex_usage("two"));
    first.emit(codex_usage("two"));
    settle().await;
    let rows = s.strings(
        "SELECT provider || '|' || account_id || '|' || cost_source FROM usage ORDER BY rowid",
    );
    assert_eq!(
        rows,
        ["codex|codex-main|estimated", "codex|codex-main|estimated"]
    );
    let second = s.captured(1);
    assert_eq!(second.resume_id, None);
    assert!(
        second.env["CLAUDE_CONFIG_DIR"].contains("claude-main"),
        "{:?}",
        second.env
    );
}

#[tokio::test]
async fn holds_the_api_account_lease_until_the_old_process_exits_before_another_bot_starts() {
    let s = stack().await;
    s.api("api", false, 1.0).await;
    let a = codex_bot(&s, json!({ "accountId": "api" }));
    let b = codex_bot(&s, json!({ "accountId": "api" }));
    s.runner.submit(&a.id, "First", &[]).unwrap();
    wait_for(|| s.sent(0) > 0).await;
    let (exit_gate, exited) = gate();
    s.session(0).set(|state| state.exit = exit_gate);
    let queued = s.runner.submit(&b.id, "Second", &[]).unwrap();
    s.session(0).emit(codex_usage("one"));
    s.session(0).emit(turn_done(true, None));
    settle().await;
    assert_eq!(s.session_count(), 1);
    assert!(s.runtime.in_use("api"));
    assert_eq!(s.message(&queued.id).status, MessageStatus::Queued);
    exited.open(());
    wait_for(|| s.sent(1) > 0).await;
    assert_eq!(
        s.captured(1).api_key,
        Some(ApiKey("synthetic-private-key".into()))
    );
    assert!(s.session(0).closed());
}

#[tokio::test]
async fn requeues_subscription_limit_work_on_an_opted_in_api_fallback_and_returns_to_a_fresh_pinned_thread_after_reset()
 {
    let s = stack().await;
    s.api("api", true, 1.0).await;
    let bot = codex_bot(&s, json!({}));
    s.runner.submit(&bot.id, "Continue this work", &[]).unwrap();
    wait_for(|| s.sent(0) > 0).await;
    s.session(0).emit(session_id("pinned-thread"));
    s.session(0).emit(limit_reached(None));
    s.session(0).emit(turn_done(false, Some("limit")));
    wait_for(|| s.sent(1) > 0).await;
    s.session(1).emit(session_id("fallback-thread"));
    settle().await;
    assert_eq!(s.captured(1).resume_id, None);
    assert_eq!(
        s.runner.to_bot(s.stored(&bot.id)).using_fallback,
        Some(true)
    );
    s.session(1).emit(codex_usage("fallback"));
    s.session(1).emit(turn_done(true, None));
    wait_for(|| !s.runtime.in_use("api")).await;
    // The subscription's limit is cleared by a newer reading.
    let observed = jiff::Timestamp::from_millisecond(jiff::Timestamp::now().as_millisecond() + 1)
        .unwrap()
        .strftime("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();
    replace_account_limits(&s.db(), "codex-main", "provider", &observed, &[]).unwrap();
    s.runner
        .submit(&bot.id, "Back on subscription", &[])
        .unwrap();
    wait_for(|| s.sent(2) > 0).await;
    let third = s.captured(2);
    assert_eq!((third.resume_id, third.api_key), (None, None));
    let shown = s.runner.to_bot(s.stored(&bot.id));
    assert_eq!(shown.effective_account_id, Some(Some("codex-main".into())));
    let saved = s.strings(&format!(
        "SELECT account_id FROM sessions WHERE bot_id = '{}' ORDER BY account_id",
        bot.id
    ));
    assert_eq!(saved, ["api", "codex-main"]);
}

#[tokio::test]
async fn explicit_stop_closes_an_idle_subscription_so_the_owner_can_change_its_sign_in() {
    let s = stack().await;
    let bot = s.bot(json!({}));
    s.runner.submit(&bot.id, "First", &[]).unwrap();
    wait_for(|| s.sent(0) > 0).await;
    s.session(0).emit(turn_done(true, None));
    settle().await;
    assert!(s.runtime.in_use("claude-main"));
    s.runner.stop(&bot.id).await.unwrap();
    assert!(s.session(0).closed());
    assert!(!s.runtime.in_use("claude-main"));
}

#[tokio::test]
async fn awaits_owned_process_retirement_records_its_final_usage_then_fences_events_after_teardown()
{
    let s = stack().await;
    s.api("api", false, 1.0).await;
    let bot = codex_bot(&s, json!({ "accountId": "api" }));
    s.runner.submit(&bot.id, "First", &[]).unwrap();
    wait_for(|| s.sent(0) > 0).await;
    let session = s.session(0);
    let (exit_gate, exited) = gate();
    session.set(|state| state.exit = exit_gate);
    let closing = s.runner.shutdown();
    let again = s.runner.shutdown();
    assert!(
        futures_util::future::Shared::ptr_eq(&closing, &again),
        "a second shutdown is the same barrier"
    );
    assert!(session.closed());
    let finished = Arc::new(AtomicBool::new(false));
    let waiting = {
        let finished = finished.clone();
        tokio::spawn(async move {
            let _ = closing.await;
            finished.store(true, Ordering::SeqCst);
        })
    };
    settle().await;
    assert!(!finished.load(Ordering::SeqCst));
    assert!(s.runtime.in_use("api"));
    // Late usage from the process still counts, against its account, until the barrier settles.
    session.emit(codex_usage("final-usage"));
    settle().await;
    assert_eq!(
        s.strings("SELECT account_id || '|' || cost_source FROM usage"),
        ["api|estimated"]
    );
    exited.open(());
    waiting.await.unwrap();
    assert!(!s.runtime.in_use("api"));
    s.runtime.close().await;
    // After teardown the runner ignores everything the old session says.
    session.emit(codex_usage("after-teardown"));
    session.emit(exit(None));
    settle().await;
    assert_eq!(s.strings("SELECT id FROM usage").len(), 1);
}

#[tokio::test]
async fn settles_an_explicit_unknown_reset_retry_from_its_real_turn_outcome() {
    for outcome in ["success", "failed", "stopped"] {
        let s = stack().await;
        let bot = s.bot(json!({}));
        s.runner.submit(&bot.id, "Queued work", &[]).unwrap();
        wait_for(|| s.sent(0) > 0).await;
        s.session(0).emit(limit_reached(None));
        s.session(0).emit(turn_done(false, Some("Provider limit")));
        wait_for(|| !s.runtime.in_use("claude-main")).await;
        assert!(!s.ready(&bot.id), "{outcome}: the limit blocks the account");
        s.runtime.retry_limit("claude-main").unwrap();
        wait_for(|| s.sent(1) > 0).await;
        let retry = s.session(1);
        if outcome == "stopped" {
            s.runner.stop(&bot.id).await.unwrap();
        }
        retry.emit(turn_done(
            outcome == "success",
            (outcome == "failed").then_some("Synthetic request failed"),
        ));
        wait_for(|| !s.runtime.in_use("claude-main")).await;
        assert!(retry.closed(), "{outcome}");
        assert_eq!(s.ready(&bot.id), outcome == "success", "{outcome}");
        let limits = s.runtime.info("claude-main").unwrap().limits.len();
        assert_eq!(
            limits,
            if outcome == "success" { 0 } else { 1 },
            "{outcome}"
        );
        assert_eq!(s.session_count(), 2, "{outcome}");
    }
}

/// `submit` leases the account and starts reading its private credential in the same synchronous prefix; the read is the
/// first await, so the pin changes in between. `assert_current` then refuses: no paid work starts, and the lease ends.
#[tokio::test]
async fn does_not_start_paid_work_when_the_pin_changes_during_credential_loading() {
    let s = stack().await;
    s.api("api", false, 1.0).await;
    let bot = codex_bot(&s, json!({ "accountId": "api" }));
    s.runner.submit(&bot.id, "First", &[]).unwrap();
    s.db()
        .execute(
            "UPDATE bots SET account_id = 'codex-main' WHERE id = ?1",
            [&bot.id],
        )
        .unwrap();
    s.runner.bot_changed(&bot.id, false).unwrap();
    wait_for(|| s.runner.current_work(&bot.id).is_none()).await;
    settle().await;
    assert_eq!(s.session_count(), 0);
    assert!(!s.runtime.in_use("api"));
}
