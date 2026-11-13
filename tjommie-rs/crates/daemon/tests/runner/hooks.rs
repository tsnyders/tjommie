//! "Runner turn hooks", "Runner session hooks" and "Runner unexpected error boundaries".

use std::collections::{BTreeMap, HashMap};

use super::*;
use daemon::chat::{NewMessage, add_message};
use daemon::providers::McpServer;
use protocol::model::{MessageKind, MessageStatus};

/// `McpTokens` from `mcp.ts` (M4): one token per bot, revoked by bot.
#[derive(Clone, Default)]
struct Tokens(Arc<Mutex<HashMap<String, String>>>);

impl Tokens {
    fn issue(&self, bot_id: &str) -> String {
        self.revoke(bot_id);
        let token = uuid::Uuid::new_v4().to_string();
        self.0
            .lock()
            .unwrap()
            .insert(token.clone(), bot_id.to_owned());
        token
    }

    fn revoke(&self, bot_id: &str) {
        self.0.lock().unwrap().retain(|_, bot| bot != bot_id);
    }

    fn bot_for(&self, token: &str) -> Option<String> {
        self.0.lock().unwrap().get(token).cloned()
    }
}

/// `mcpServers` issues a token per session and `sessionClosed` revokes it, as `daemon.ts` wires them.
fn token_hooks(
    tokens: &Tokens,
    issued: &Recorded<String>,
    closed: &Recorded<String>,
    servers: bool,
) -> RunnerHooks {
    let (issuer, revoker, issued, closed) = (
        tokens.clone(),
        tokens.clone(),
        issued.clone(),
        closed.clone(),
    );
    RunnerHooks {
        mcp_servers: Some(Arc::new(move |_core, bot: &StoredBot, _item: &WorkItem| {
            let token = issuer.issue(&bot.id);
            issued.lock().unwrap().push(token.clone());
            Ok(if servers {
                vec![tjommie_server(&token)]
            } else {
                Vec::new()
            })
        })),
        session_closed: Some(Arc::new(move |_core, bot_id: &str| {
            closed.lock().unwrap().push(bot_id.to_owned());
            revoker.revoke(bot_id);
            Ok(())
        })),
        ..RunnerHooks::default()
    }
}

fn tjommie_server(token: &str) -> McpServer {
    McpServer::Http {
        name: "tjommie".into(),
        url: "http://127.0.0.1:47100/mcp".into(),
        headers: BTreeMap::from([("Authorization".to_owned(), format!("Bearer {token}"))]),
        auto_allow: true,
    }
}

/// Session hooks plus the lifecycle recorder.
fn with_lifecycle(hooks: RunnerHooks, lifecycle: &Lifecycle) -> RunnerHooks {
    RunnerHooks {
        mcp_servers: hooks.mcp_servers,
        session_closed: hooks.session_closed,
        ..lifecycle.hooks()
    }
}

fn result(
    outcome: TurnOutcome,
    text: Option<&str>,
    text_message_id: Option<String>,
    error: Option<&str>,
) -> TurnResult {
    TurnResult {
        outcome,
        text: text.map(Into::into),
        text_message_id,
        error: error.map(Into::into),
    }
}

#[tokio::test]
async fn reports_the_outcome_with_its_last_finished_nonblank_text() {
    for outcome in [
        TurnOutcome::Done,
        TurnOutcome::Failed,
        TurnOutcome::Interrupted,
    ] {
        let lifecycle = Lifecycle::default();
        let h = harness_with(Setup {
            hooks: lifecycle.hooks(),
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        let item = h
            .runner
            .deliver(work_in(&bot.id, WorkSource::BotRequest, "reply", "team"))
            .unwrap();
        assert_eq!(lifecycle.started(), std::slice::from_ref(&item.id));
        wait_for(|| h.sent(0) > 0).await;
        let session = h.session(0);
        session.emit(text("first thought"));
        session.emit(ProviderEvent::TextEnd);
        session.emit(text("finished answer"));
        session.emit(ProviderEvent::TextEnd);
        settle().await;
        let finished = h.last("team");
        session.emit(text("  "));
        session.emit(ProviderEvent::TextEnd);
        session.emit(text("final fragment"));
        settle().await;
        if outcome == TurnOutcome::Interrupted {
            h.runner.stop(&bot.id).await.unwrap();
        }
        let error = (outcome == TurnOutcome::Failed).then_some("failed after answer");
        session.emit(turn_done(outcome == TurnOutcome::Done, error));
        settle().await;
        let last = h
            .history("team")
            .into_iter()
            .find(|message| message.body == "final fragment")
            .unwrap();
        let expected_status = match outcome {
            TurnOutcome::Done => MessageStatus::Done,
            TurnOutcome::Failed => MessageStatus::Failed,
            TurnOutcome::Interrupted => MessageStatus::Interrupted,
        };
        assert_eq!(last.status, expected_status);
        let ended = lifecycle.ended();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].0.id, item.id);
        let expected = if outcome == TurnOutcome::Done {
            result(
                outcome,
                Some("final fragment"),
                Some(last.id.clone()),
                error,
            )
        } else {
            result(
                outcome,
                Some("finished answer"),
                Some(finished.id.clone()),
                error,
            )
        };
        assert_eq!(ended[0].1, expected);
        session.emit(turn_done(false, Some("duplicate terminal event")));
        settle().await;
        assert_eq!(lifecycle.ended().len(), 1);
    }
}

#[tokio::test]
async fn resets_result_text_between_turns_and_leaves_provider_started_turns_without_a_work_item() {
    let lifecycle = Lifecycle::default();
    let h = harness_with(Setup {
        hooks: lifecycle.hooks(),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "first", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let session = h.session(0);
    session.emit(text("answer one"));
    session.emit(turn_done(true, None));
    settle().await;
    h.runner.submit(&bot.id, "second", &[]).unwrap();
    wait_for(|| session.sent().len() == 2).await;
    session.emit(turn_done(true, None));
    settle().await;
    assert_eq!(
        lifecycle.results()[1],
        result(TurnOutcome::Done, None, None, None)
    );
    session.emit(text("background answer"));
    settle().await;
    assert_eq!(h.runner.current_work(&bot.id), None);
    session.emit(turn_done(true, None));
    settle().await;
    assert_eq!(lifecycle.started().len(), 2);
    assert_eq!(lifecycle.ended().len(), 2);
    let last = h.last(&bot.id);
    assert_eq!(
        (last.body.as_str(), last.status),
        ("background answer", MessageStatus::Done)
    );
    h.runner.submit(&bot.id, "third", &[]).unwrap();
    wait_for(|| session.sent().len() == 3).await;
    session.emit(turn_done(true, None));
    settle().await;
    assert_eq!(
        lifecycle.results()[2],
        result(TurnOutcome::Done, None, None, None)
    );
}

#[tokio::test]
async fn calls_turn_requeued_instead_of_turn_ended_for_a_failed_limit_turn_and_starts_its_same_item_again()
 {
    let lifecycle = Lifecycle::default();
    let h = harness_with(Setup {
        hooks: lifecycle.hooks(),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let message = h.runner.submit(&bot.id, "wait through limit", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let session = h.session(0);
    let item_id = h.item_for_message(&message.id).id;
    session.emit(limit_reached(Some(FAR_RESET)));
    session.emit(turn_done(false, Some("Usage limit reached")));
    settle().await;
    assert_eq!(h.status(&item_id), WorkStatus::Queued);
    assert_eq!(h.message(&message.id).status, MessageStatus::Queued);
    assert_eq!(lifecycle.requeued(), std::slice::from_ref(&item_id));
    assert_eq!(lifecycle.ended().len(), 0);
    assert_eq!(h.runner.current_work(&bot.id), None);
    h.db()
        .execute(
            "UPDATE bots SET limited_until = NULL WHERE id = ?1",
            [&bot.id],
        )
        .unwrap();
    h.runner.pump(&bot.id);
    wait_for(|| session.sent().len() == 2).await;
    assert_eq!(lifecycle.started(), [item_id.clone(), item_id.clone()]);
    session.emit(turn_done(true, None));
    settle().await;
    assert_eq!(lifecycle.requeued().len(), 1);
    let ended = lifecycle.ended();
    assert_eq!(ended.len(), 1);
    assert_eq!(
        (ended[0].0.id.as_str(), ended[0].1.outcome),
        (item_id.as_str(), TurnOutcome::Done)
    );
}

#[tokio::test]
async fn ends_a_limited_turn_normally_after_success_or_stop_without_a_requeue_callback() {
    for stop in [false, true] {
        let lifecycle = Lifecycle::default();
        let h = harness_with(Setup {
            hooks: lifecycle.hooks(),
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        let message = h.runner.submit(&bot.id, "finish once", &[]).unwrap();
        wait_for(|| h.sent(0) > 0).await;
        let session = h.session(0);
        session.emit(limit_reached(Some(FAR_RESET)));
        settle().await;
        if stop {
            h.runner.stop(&bot.id).await.unwrap();
        }
        session.emit(turn_done(!stop, None));
        settle().await;
        let expected = if stop {
            TurnOutcome::Interrupted
        } else {
            TurnOutcome::Done
        };
        let item = h.item_for_message(&message.id);
        assert_eq!(
            item.status,
            if stop {
                WorkStatus::Interrupted
            } else {
                WorkStatus::Done
            }
        );
        let outcomes: Vec<(String, TurnOutcome)> = lifecycle
            .ended()
            .into_iter()
            .map(|(work, result)| (work.id, result.outcome))
            .collect();
        assert_eq!(outcomes, [(item.id.clone(), expected)]);
        assert_eq!(lifecycle.requeued(), Vec::<String>::new());
        h.db()
            .execute(
                "UPDATE bots SET limited_until = NULL WHERE id = ?1",
                [&bot.id],
            )
            .unwrap();
        h.runner.pump(&bot.id);
        settle().await;
        assert_eq!(session.sent().len(), 1);
    }
}

#[tokio::test]
async fn honors_stop_item_called_synchronously_by_turn_started_before_creating_the_session() {
    let ended: Recorded<TurnResult> = Arc::default();
    let record = ended.clone();
    let hooks = RunnerHooks {
        turn_started: Some(Arc::new(|core, item: &WorkItem| {
            drop(core.stop_item(&item.id, "Already out of time."));
            Ok(())
        })),
        turn_ended: Some(Arc::new(move |_core, _item, result: &TurnResult| {
            record.lock().unwrap().push(result.clone());
            Ok(())
        })),
        ..RunnerHooks::default()
    };
    let h = harness_with(Setup {
        hooks,
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    let item = h
        .runner
        .deliver(work_in(&bot.id, WorkSource::Routine, "do not send", "team"))
        .unwrap();
    wait_for(|| !recorded(&ended).is_empty()).await;
    assert_eq!(h.status(&item.id), WorkStatus::Interrupted);
    assert!(
        recorded(&h.sessions)
            .iter()
            .all(|session| session.sent().is_empty())
    );
    assert_eq!(h.last("team").body, "Already out of time.");
}

#[tokio::test]
async fn logs_a_throwing_hook_without_leaving_the_bot_stuck() {
    for boundary in ["turnStarted", "turnEnded", "turnRequeued", "sessionClosed"] {
        let failure = format!("{boundary} failed");
        let fail = failure.clone();
        let hooks = match boundary {
            "turnStarted" => RunnerHooks {
                turn_started: Some(Arc::new(move |_core, _item| Err(fail.clone()))),
                ..RunnerHooks::default()
            },
            "turnEnded" => RunnerHooks {
                turn_ended: Some(Arc::new(move |_core, _item, _result| Err(fail.clone()))),
                ..RunnerHooks::default()
            },
            "turnRequeued" => RunnerHooks {
                turn_requeued: Some(Arc::new(move |_core, _item| Err(fail.clone()))),
                ..RunnerHooks::default()
            },
            _ => RunnerHooks {
                session_closed: Some(Arc::new(move |_core, _bot| Err(fail.clone()))),
                ..RunnerHooks::default()
            },
        };
        let h = harness_with(Setup {
            hooks,
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        let message = h.runner.submit(&bot.id, "first", &[]).unwrap();
        wait_for(|| h.sent(0) > 0).await;
        let session = h.session(0);
        if boundary == "turnRequeued" {
            session.emit(limit_reached(Some(FAR_RESET)));
        }
        if boundary == "sessionClosed" {
            session.emit(exit(Some("session died")));
        } else {
            session.emit(turn_done(boundary != "turnRequeued", None));
        }
        settle().await;
        assert!(h.logged(&failure), "{boundary}: {:?}", recorded(&h.logs));
        assert_eq!(h.runner.current_work(&bot.id), None, "{boundary}");
        assert_eq!(
            h.state(&bot.id),
            if boundary == "turnRequeued" {
                BotState::Limited
            } else {
                BotState::Idle
            }
        );
        let expected = match boundary {
            "turnRequeued" => WorkStatus::Queued,
            "sessionClosed" => WorkStatus::Failed,
            _ => WorkStatus::Done,
        };
        assert_eq!(
            h.item_for_message(&message.id).status,
            expected,
            "{boundary}"
        );
        h.db()
            .execute(
                "UPDATE bots SET limited_until = NULL WHERE id = ?1",
                [&bot.id],
            )
            .unwrap();
        if boundary == "turnRequeued" {
            h.runner.pump(&bot.id);
        } else {
            h.runner.submit(&bot.id, "next", &[]).unwrap();
        }
        wait_for(|| {
            recorded(&h.sessions)
                .iter()
                .map(|active| active.sent().len())
                .sum::<usize>()
                == 2
        })
        .await;
        assert!(h.runner.current_work(&bot.id).is_some(), "{boundary}");
    }
}

#[tokio::test]
async fn settles_stop_during_a_rejected_resumed_start_without_a_fresh_attempt_and_revokes_its_token()
 {
    let session = FakeSession::new();
    let (start, reject_start) = gate();
    session.set(|state| state.start = start);
    let options: Arc<Mutex<Vec<StartOptions>>> = Arc::default();
    let captured = options.clone();
    let reuse = session.clone();
    let factory: Factory = Arc::new(move |start| {
        captured.lock().unwrap().push(start.clone());
        Ok(reuse.clone())
    });
    let (tokens, issued, closed) = (
        Tokens::default(),
        Recorded::<String>::default(),
        Recorded::<String>::default(),
    );
    let lifecycle = Lifecycle::default();
    let hooks = with_lifecycle(token_hooks(&tokens, &issued, &closed, false), &lifecycle);
    let h = harness_with(Setup {
        factory,
        hooks,
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    h.db()
        .execute("INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at) VALUES (?1, 'claude', 'claude-main', 'saved', 'then')", [&bot.id])
        .unwrap();
    let message = h.runner.submit(&bot.id, "never send", &[]).unwrap();
    h.runner.stop(&bot.id).await.unwrap();
    reject_start.open(Err("start rejected after Stop".into()));
    wait_for(|| !lifecycle.ended().is_empty()).await;
    let resume_ids: Vec<Option<String>> = recorded(&options)
        .iter()
        .map(|option| option.resume_id.clone())
        .collect();
    assert_eq!(resume_ids, [Some("saved".to_owned())]);
    assert_eq!(h.session_count(), 1);
    assert_eq!(session.sent(), Vec::<String>::new());
    assert!(session.closed());
    assert_eq!(
        h.item_for_message(&message.id).status,
        WorkStatus::Interrupted
    );
    assert_eq!(
        lifecycle.results()[0],
        result(TurnOutcome::Interrupted, None, None, None)
    );
    assert_eq!(tokens.bot_for(&recorded(&issued)[0]), None);
    assert_eq!(*recorded(&closed), std::slice::from_ref(&bot.id));
    assert_eq!(h.saved_session(&bot.id).as_deref(), Some("saved"));
    assert!(
        !h.history(&bot.id)
            .iter()
            .any(|item| item.kind == MessageKind::Error)
    );
}

#[tokio::test]
async fn brackets_the_same_resumed_items_fallback_with_requeue_and_start_hooks() {
    for (boundary, throws) in [
        ("exit", false),
        ("start", false),
        ("exit", true),
        ("start", true),
    ] {
        let resumed = FakeSession::new();
        let (start, reject_start) = gate();
        if boundary == "start" {
            resumed.set(|state| state.start = start);
        }
        let fresh = FakeSession::new();
        let lifecycle = Lifecycle::default();
        let mut hooks = lifecycle.hooks();
        if throws {
            let requeued = lifecycle.0.clone();
            hooks.turn_requeued = Some(Arc::new(move |_core, item: &WorkItem| {
                requeued
                    .lock()
                    .unwrap()
                    .push(("requeued", item.clone(), None));
                Err("requeue hook failed".to_owned())
            }));
        }
        let h = harness_with(Setup {
            factory: sequence(vec![resumed.clone(), fresh.clone()]),
            hooks,
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        h.db()
            .execute("INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at) VALUES (?1, 'claude', 'claude-main', 'missing', 'then')", [&bot.id])
            .unwrap();
        let message = h
            .runner
            .submit(&bot.id, "continue the same item", &[])
            .unwrap();
        let item_id = h.item_for_message(&message.id).id;
        assert_eq!(lifecycle.all(), [("started", item_id.clone())]);
        if boundary == "exit" {
            wait_for(|| !resumed.sent().is_empty()).await;
            resumed.emit(exit(None));
        } else {
            reject_start.open(Err("resume start rejected".into()));
        }
        wait_for(|| !fresh.sent().is_empty()).await;
        let bracket = [
            ("started", item_id.clone()),
            ("requeued", item_id.clone()),
            ("started", item_id.clone()),
        ];
        assert_eq!(lifecycle.all(), bracket, "{boundary} {throws}");
        assert_eq!(lifecycle.ended().len(), 0);
        assert_eq!(
            h.runner.current_work(&bot.id).map(|work| work.id),
            Some(item_id.clone())
        );
        assert_eq!(h.status(&item_id), WorkStatus::Running);
        if throws {
            assert!(h.logged("requeue hook failed"));
        } else {
            assert!(h.no_logs(), "{:?}", recorded(&h.logs));
        }
        fresh.emit(text("final answer"));
        fresh.emit(turn_done(true, None));
        settle().await;
        let mut done = bracket.to_vec();
        done.push(("ended", item_id.clone()));
        assert_eq!(lifecycle.all(), done);
        let last_id = h.last(&bot.id).id;
        assert_eq!(
            lifecycle.results(),
            [result(
                TurnOutcome::Done,
                Some("final answer"),
                Some(last_id),
                None
            )]
        );
        assert_eq!(h.status(&item_id), WorkStatus::Done);
        assert_eq!(h.session_count(), 2);
    }
}

#[tokio::test]
async fn retries_a_rejected_resumed_start_only_once_with_a_renewed_token() {
    for fresh_fails in [false, true] {
        let options: Arc<Mutex<Vec<StartOptions>>> = Arc::default();
        let captured = options.clone();
        let factory: Factory = Arc::new(move |start| {
            let mut options = captured.lock().unwrap();
            options.push(start.clone());
            let session = FakeSession::new();
            if options.len() == 1 || fresh_fails {
                let error = if options.len() == 1 {
                    "resume rejected"
                } else {
                    "fresh rejected"
                };
                session.set(|state| state.start = ready(Err(error.into())));
            }
            Ok(session)
        });
        let (tokens, issued, closed) = (
            Tokens::default(),
            Recorded::<String>::default(),
            Recorded::<String>::default(),
        );
        let lifecycle = Lifecycle::default();
        let hooks = with_lifecycle(token_hooks(&tokens, &issued, &closed, false), &lifecycle);
        let h = harness_with(Setup {
            factory,
            hooks,
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        add_message(
            &h.db(),
            &NewMessage {
                conversation_id: bot.id.clone(),
                author: "you".into(),
                kind: MessageKind::Text,
                body: "Remember ZEBRA".into(),
                data: None,
                status: MessageStatus::Done,
            },
        )
        .unwrap();
        h.db()
            .execute("INSERT INTO sessions (bot_id, provider, account_id, cli_session_id, updated_at) VALUES (?1, 'claude', 'claude-main', 'missing', 'then')", [&bot.id])
            .unwrap();
        let message = h.runner.submit(&bot.id, "continue", &[]).unwrap();
        if fresh_fails {
            wait_for(|| !lifecycle.ended().is_empty()).await;
        } else {
            wait_for(|| h.sent(1) > 0).await;
        }
        let resume_ids: Vec<Option<String>> = recorded(&options)
            .iter()
            .map(|option| option.resume_id.clone())
            .collect();
        assert_eq!(resume_ids, [Some("missing".to_owned()), None]);
        assert_eq!(h.session_count(), 2);
        assert!(h.session(0).closed());
        let tokens_issued = recorded(&issued).clone();
        assert_eq!(tokens.bot_for(&tokens_issued[0]), None);
        assert_eq!(h.saved_session(&bot.id), None);
        if fresh_fails {
            assert_eq!(*recorded(&closed), [bot.id.clone(), bot.id.clone()]);
            assert_eq!(tokens.bot_for(&tokens_issued[1]), None);
            assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Failed);
            assert_eq!(
                lifecycle.results()[0].error.as_deref(),
                Some("fresh rejected")
            );
            assert_eq!(h.runner.current_work(&bot.id), None);
        } else {
            assert_eq!(*recorded(&closed), std::slice::from_ref(&bot.id));
            assert_eq!(tokens.bot_for(&tokens_issued[1]), Some(bot.id.clone()));
            let opening = h.session(1).sent()[0].clone();
            assert!(opening.contains("Person: Remember ZEBRA"));
            assert!(opening.contains("continue"));
            assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Running);
            assert_eq!(lifecycle.ended().len(), 0);
            h.session(1).emit(exit(None));
            settle().await;
            assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Failed);
            assert_eq!(h.session_count(), 2);
            assert_eq!(tokens.bot_for(&tokens_issued[1]), None);
        }
    }
}

#[tokio::test]
async fn constructs_mcp_servers_once_per_session_and_revokes_the_old_token_when_settings_close_it()
{
    let options: Arc<Mutex<Vec<StartOptions>>> = Arc::default();
    let (tokens, issued, closed) = (
        Tokens::default(),
        Recorded::<String>::default(),
        Recorded::<String>::default(),
    );
    let h = harness_with(Setup {
        factory: capturing(&options),
        hooks: token_hooks(&tokens, &issued, &closed, true),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "first", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let first_token = recorded(&issued)[0].clone();
    assert_eq!(
        recorded(&options)[0].mcp_servers,
        [tjommie_server(&first_token)]
    );
    assert_eq!(tokens.bot_for(&first_token), Some(bot.id.clone()));
    h.session(0).emit(turn_done(true, None));
    settle().await;
    h.runner.submit(&bot.id, "reuse", &[]).unwrap();
    wait_for(|| h.session(0).sent().len() == 2).await;
    assert_eq!(recorded(&issued).len(), 1);
    h.runner.bot_changed(&bot.id, false).unwrap();
    assert_eq!(*recorded(&closed), Vec::<String>::new());
    h.session(0).emit(turn_done(true, None));
    settle().await;
    assert_eq!(*recorded(&closed), std::slice::from_ref(&bot.id));
    assert_eq!(tokens.bot_for(&first_token), None);
    h.runner.submit(&bot.id, "new settings", &[]).unwrap();
    wait_for(|| h.sent(1) > 0).await;
    let second_token = recorded(&issued)[1].clone();
    assert_eq!(recorded(&issued).len(), 2);
    assert_eq!(tokens.bot_for(&second_token), Some(bot.id.clone()));
    h.runner.forget(&bot.id);
    assert_eq!(tokens.bot_for(&second_token), None);
    assert_eq!(*recorded(&closed), [bot.id.clone(), bot.id.clone()]);
    h.session(1).emit(exit(None));
    settle().await;
    assert_eq!(recorded(&closed).len(), 2);
}

#[tokio::test]
async fn revokes_an_issued_mcp_token_when_startup_fails() {
    for boundary in ["factory", "start", "mcpServers"] {
        let failure = format!("{boundary} startup failed");
        let message_text = failure.clone();
        let factory: Factory = Arc::new(move |_| {
            if boundary == "factory" {
                return Err(message_text.clone());
            }
            let session = FakeSession::new();
            session.set(|state| state.start = ready(Err(message_text.clone())));
            Ok(session)
        });
        let (tokens, issued, closed) = (
            Tokens::default(),
            Recorded::<String>::default(),
            Recorded::<String>::default(),
        );
        let lifecycle = Lifecycle::default();
        let mut hooks = with_lifecycle(token_hooks(&tokens, &issued, &closed, false), &lifecycle);
        if boundary == "mcpServers" {
            let (issuer, issued, fail) = (tokens.clone(), issued.clone(), failure.clone());
            hooks.mcp_servers = Some(Arc::new(move |_core, bot: &StoredBot, _item| {
                issued.lock().unwrap().push(issuer.issue(&bot.id));
                Err(fail.clone())
            }));
        }
        let h = harness_with(Setup {
            factory,
            hooks,
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        let message = h.runner.submit(&bot.id, "cannot start", &[]).unwrap();
        wait_for(|| !lifecycle.ended().is_empty()).await;
        let token = recorded(&issued)[0].clone();
        assert!(!token.is_empty());
        assert_eq!(tokens.bot_for(&token), None, "{boundary}");
        assert_eq!(
            *recorded(&closed),
            std::slice::from_ref(&bot.id),
            "{boundary}"
        );
        assert!(
            recorded(&h.sessions)
                .iter()
                .all(|session| session.sent().is_empty() && session.closed())
        );
        assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Failed);
        assert_eq!(
            lifecycle.results()[0],
            result(TurnOutcome::Failed, None, None, Some(&failure))
        );
        assert_eq!(h.state(&bot.id), BotState::Idle);
        if boundary == "mcpServers" {
            assert!(h.logged(&failure));
        }
    }
}

#[tokio::test]
async fn revokes_once_when_a_live_session_exits_and_ignores_later_events_from_that_session() {
    let (tokens, issued, closed) = (
        Tokens::default(),
        Recorded::<String>::default(),
        Recorded::<String>::default(),
    );
    let h = harness_with(Setup {
        hooks: token_hooks(&tokens, &issued, &closed, false),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "first", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let old = h.session(0);
    old.emit(exit(None));
    settle().await;
    assert_eq!(*recorded(&closed), std::slice::from_ref(&bot.id));
    h.runner.submit(&bot.id, "next", &[]).unwrap();
    wait_for(|| h.sent(1) > 0).await;
    old.emit(exit(Some("late duplicate")));
    old.emit(text("stale"));
    settle().await;
    assert_eq!(*recorded(&closed), std::slice::from_ref(&bot.id));
    assert_eq!(
        h.runner.current_work(&bot.id).map(|work| work.text),
        Some("next".to_owned())
    );
    assert!(
        !h.history(&bot.id)
            .iter()
            .any(|message| message.body == "stale")
    );
}

#[tokio::test]
async fn adds_exactly_the_last_20_chat_messages_only_for_a_fresh_session() {
    let options: Arc<Mutex<Vec<StartOptions>>> = Arc::default();
    let h = harness_with(Setup {
        factory: capturing(&options),
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    for index in 0..25 {
        add_message(
            &h.db(),
            &NewMessage {
                conversation_id: bot.id.clone(),
                author: "you".into(),
                kind: MessageKind::Text,
                body: format!("history-{index}"),
                data: None,
                status: MessageStatus::Sent,
            },
        )
        .unwrap();
    }
    h.runner.submit(&bot.id, "fresh request", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    let session = h.session(0);
    let opening = session.sent()[0].clone();
    let lines: Vec<&str> = opening
        .split('\n')
        .filter(|line| line.starts_with("Person: history-"))
        .collect();
    let expected: Vec<String> = (5..25)
        .map(|index| format!("Person: history-{index}"))
        .collect();
    assert_eq!(lines, expected);
    assert!(opening.contains("fresh request"));
    session.emit(session_id("remembered"));
    session.emit(turn_done(true, None));
    settle().await;
    h.runner.submit(&bot.id, "same session", &[]).unwrap();
    wait_for(|| session.sent().len() == 2).await;
    assert_eq!(session.sent()[1], "same session");
    session.emit(turn_done(true, None));
    settle().await;
    h.runner.bot_changed(&bot.id, false).unwrap();
    h.runner.submit(&bot.id, "resumed request", &[]).unwrap();
    wait_for(|| h.sent(1) > 0).await;
    assert_eq!(
        recorded(&options)[1].resume_id.as_deref(),
        Some("remembered")
    );
    assert_eq!(h.session(1).sent(), ["resumed request"]);
}

#[tokio::test]
async fn reports_a_rejected_stop_item_interrupt_so_its_caller_can_clear_the_time_limit() {
    let h = harness();
    let bot = h.bot(json!({}));
    let item = h
        .runner
        .deliver(work(&bot.id, WorkSource::Routine, "run"))
        .unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0)
        .set(|state| state.interrupt_error = Some("interrupt transport failed".into()));
    let error = h
        .runner
        .stop_item(&item.id, "Routine timed out.")
        .await
        .unwrap_err();
    assert_eq!(error.message(), "interrupt transport failed");
    h.session(0)
        .emit(turn_done(false, Some("The command failed.")));
    settle().await;
    assert_eq!(h.status(&item.id), WorkStatus::Failed);
    assert_eq!(h.last(&bot.id).body, "The command failed.");
    assert_eq!(h.runner.current_work(&bot.id), None);
}

#[tokio::test]
async fn logs_an_unexpected_bot_broadcast_error_and_still_starts_and_settles_the_work() {
    for failure in ["bot broadcast failed", "broadcast target missing"] {
        let on_broadcast: OnBroadcast = Arc::new(move |event| {
            if matches!(event, Event::BotUpdated(_)) {
                Err(failure.to_owned())
            } else {
                Ok(())
            }
        });
        let h = harness_with(Setup {
            on_broadcast: Some(on_broadcast),
            ..Setup::default()
        });
        let bot = h.bot(json!({}));
        let message = h.runner.submit(&bot.id, "still run", &[]).unwrap();
        wait_for(|| h.sent(0) > 0).await;
        h.session(0).emit(turn_done(true, None));
        settle().await;
        assert!(h.logged(failure));
        assert_eq!(h.item_for_message(&message.id).status, WorkStatus::Done);
        assert_eq!(h.runner.current_work(&bot.id), None);
    }
}

#[tokio::test]
async fn does_not_log_a_bot_deleted_by_the_turn_ended_hook_as_an_unexpected_error() {
    let hooks = RunnerHooks {
        turn_ended: Some(Arc::new(|core, item: &WorkItem, _result| {
            core.db()
                .execute("DELETE FROM bots WHERE id = ?1", [&item.bot_id])
                .map(drop)
                .map_err(|error| error.to_string())
        })),
        ..RunnerHooks::default()
    };
    let h = harness_with(Setup {
        hooks,
        ..Setup::default()
    });
    let bot = h.bot(json!({}));
    h.runner.submit(&bot.id, "last turn", &[]).unwrap();
    wait_for(|| h.sent(0) > 0).await;
    h.session(0).emit(turn_done(true, None));
    settle().await;
    assert_eq!(
        h.count(&format!(
            "SELECT count(*) FROM bots WHERE id = '{}'",
            bot.id
        )),
        0
    );
    assert!(h.no_logs(), "{:?}", recorded(&h.logs));
    assert_eq!(h.runner.current_work(&bot.id), None);
}
