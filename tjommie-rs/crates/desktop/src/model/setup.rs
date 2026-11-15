//! Port of `packages/ui/lib/setup.ts` (T1.4): what each provider's setup card shows, and which sign-ins are still going.

use std::borrow::Cow;
use std::collections::HashMap;

use protocol::model::{CodexSandboxReadiness, CodexSetup, Provider, ProviderSetup, SetupFields};

/// The names the cards show (`PROVIDER_NAME`).
pub fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "Claude Code",
        Provider::Codex => "Codex",
    }
}

/// The fields every provider's report has, whichever provider it is.
pub fn report_of(setup: &ProviderSetup) -> &SetupFields {
    match setup {
        ProviderSetup::Claude(fields) => fields,
        ProviderSetup::Codex(codex) => &codex.setup,
    }
}

fn provider_of(setup: &ProviderSetup) -> Provider {
    match setup {
        ProviderSetup::Claude(_) => Provider::Claude,
        ProviderSetup::Codex(_) => Provider::Codex,
    }
}

/// JavaScript's truthiness for the optional strings: an empty one counts as missing.
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

fn sandbox_view(readiness: CodexSandboxReadiness, preparing: bool) -> SandboxView {
    match readiness {
        CodexSandboxReadiness::Ready => SandboxView::Ready,
        _ if preparing => SandboxView::Preparing,
        CodexSandboxReadiness::Unavailable => SandboxView::Unavailable,
        CodexSandboxReadiness::NotConfigured | CodexSandboxReadiness::UpdateRequired => {
            SandboxView::Needed
        }
    }
}

/// Codex's sandbox as its card shows it. `Preparing` only while this window is setting it up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxView {
    Ready,
    Needed,
    Preparing,
    Unavailable,
}

/// What a provider's setup card shows (WF9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardView {
    Checking,
    NotInstalled,
    SigningIn,
    SignedIn {
        account: Option<String>,
        plan: Option<String>,
        sandbox: Option<SandboxView>,
    },
    Problem {
        reason: String,
    },
    SignedOut,
}

/// What this window is doing for a provider: `signing_in` since Sign in was clicked, `preparing` since Set up was.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalState {
    pub signing_in: bool,
    pub preparing: bool,
}

/// The card for a provider's report (`undefined` until the first check comes back).
pub fn card_view(setup: Option<&ProviderSetup>, local: LocalState) -> CardView {
    let Some(setup) = setup else {
        return CardView::Checking;
    };
    let report = report_of(setup);
    if !report.installed {
        return CardView::NotInstalled;
    }
    if local.signing_in {
        return CardView::SigningIn;
    }
    if report.signed_in {
        let sandbox = match setup {
            ProviderSetup::Claude(_) => None,
            ProviderSetup::Codex(codex) => Some(sandbox_view(codex.sandbox, local.preparing)),
        };
        return CardView::SignedIn {
            account: report.account.clone(),
            plan: report.plan.clone(),
            sandbox,
        };
    }
    match non_empty(report.error.as_deref()) {
        Some(reason) => CardView::Problem {
            reason: reason.to_owned(),
        },
        None => CardView::SignedOut,
    }
}

/// Signed in or not, and the account: what "the same sign-in" compares. A missing report only matches a missing one.
fn sign_in_of(setup: Option<&ProviderSetup>) -> Option<(bool, Option<&str>)> {
    setup.map(|setup| {
        let report = report_of(setup);
        (report.signed_in, report.account.as_deref())
    })
}

/// Whether two reports of a provider say the same about its sign-in: signed in or not, and the account.
pub fn same_sign_in(a: Option<&ProviderSetup>, b: Option<&ProviderSetup>) -> bool {
    sign_in_of(a) == sign_in_of(b)
}

/// A sign-in in progress: the report when Sign in was clicked, and the daemon's id for that attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct SignIn {
    pub since: Option<ProviderSetup>,
    pub attempt: String,
}

/// Each provider's sign-in in progress.
pub type SignIns = HashMap<Provider, SignIn>;

/// Drops the sign-ins that are over: their attempt ended, or the provider's report changed. Borrows `sign_ins` back
/// when none is dropped, so the caller can tell by identity, as the TypeScript returns the same object.
pub fn live_sign_ins<'a>(
    sign_ins: &'a SignIns,
    ended: &[String],
    setup: Option<&[ProviderSetup]>,
) -> Cow<'a, SignIns> {
    let live: SignIns = sign_ins
        .iter()
        .filter(|(provider, entry)| {
            let current = setup.and_then(|reports| {
                reports
                    .iter()
                    .find(|report| provider_of(report) == **provider)
            });
            !ended.contains(&entry.attempt) && same_sign_in(current, entry.since.as_ref())
        })
        .map(|(provider, entry)| (*provider, entry.clone()))
        .collect();
    if live.len() == sign_ins.len() {
        Cow::Borrowed(sign_ins)
    } else {
        Cow::Owned(live)
    }
}

/// Both providers' reports when the setup check itself fails, so each card shows the reason and Check again.
pub fn failed_setup(reason: &str) -> Vec<ProviderSetup> {
    let failed = SetupFields {
        installed: true,
        version: None,
        signed_in: false,
        account: None,
        plan: None,
        error: Some(reason.to_owned()),
    };
    vec![
        ProviderSetup::Claude(failed.clone()),
        ProviderSetup::Codex(CodexSetup {
            setup: failed,
            sandbox: CodexSandboxReadiness::Unavailable,
        }),
    ]
}

/// "Max plan", from the CLI's plan name. The first letter is upper-cased as JavaScript's `toUpperCase` does it.
pub fn plan_label(plan: Option<&str>) -> Option<String> {
    let mut letters = non_empty(plan)?.chars();
    let first = letters.next()?;
    Some(format!("{}{} plan", first.to_uppercase(), letters.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUIET: LocalState = LocalState {
        signing_in: false,
        preparing: false,
    };

    const SIGNING_IN: LocalState = LocalState {
        signing_in: true,
        preparing: false,
    };

    /// A Claude report as the TypeScript fixture builds it: installed, signed out, then `edit`ed.
    fn claude(edit: impl FnOnce(&mut SetupFields)) -> ProviderSetup {
        let mut fields = SetupFields {
            installed: true,
            version: Some("2.1.280".to_owned()),
            signed_in: false,
            account: None,
            plan: None,
            error: None,
        };
        edit(&mut fields);
        ProviderSetup::Claude(fields)
    }

    /// A Codex report with its sandbox at `sandbox`, then `edit`ed.
    fn codex(sandbox: CodexSandboxReadiness, edit: impl FnOnce(&mut SetupFields)) -> ProviderSetup {
        let mut fields = SetupFields {
            installed: true,
            version: Some("0.157.1".to_owned()),
            signed_in: false,
            account: None,
            plan: None,
            error: None,
        };
        edit(&mut fields);
        ProviderSetup::Codex(CodexSetup {
            setup: fields,
            sandbox,
        })
    }

    fn sandbox_of(setup: &ProviderSetup, local: LocalState) -> Option<SandboxView> {
        match card_view(Some(setup), local) {
            CardView::SignedIn { sandbox, .. } => sandbox,
            _ => None,
        }
    }

    #[test]
    fn shows_checking_not_installed_a_failed_check_signed_out_and_signing_in() {
        assert_eq!(card_view(None, QUIET), CardView::Checking);
        let not_installed = claude(|fields| {
            fields.installed = false;
            fields.version = None;
            fields.error = Some("Claude Code is not installed".to_owned());
        });
        assert_eq!(
            card_view(Some(&not_installed), QUIET),
            CardView::NotInstalled
        );
        let failed = claude(|fields| fields.error = Some("Claude sign-in check failed".to_owned()));
        assert_eq!(
            card_view(Some(&failed), QUIET),
            CardView::Problem {
                reason: "Claude sign-in check failed".to_owned()
            }
        );
        let signed_out = claude(|_| {});
        assert_eq!(card_view(Some(&signed_out), QUIET), CardView::SignedOut);
        assert_eq!(
            card_view(Some(&signed_out), SIGNING_IN),
            CardView::SigningIn
        );
        let signed_in = claude(|fields| fields.signed_in = true);
        assert_eq!(card_view(Some(&signed_in), SIGNING_IN), CardView::SigningIn);
    }

    #[test]
    fn shows_the_account_and_whether_codexs_sandbox_still_needs_setting_up() {
        let account = claude(|fields| {
            fields.signed_in = true;
            fields.account = Some("owner@example.com".to_owned());
            fields.plan = Some("max".to_owned());
        });
        assert_eq!(
            card_view(Some(&account), QUIET),
            CardView::SignedIn {
                account: Some("owner@example.com".to_owned()),
                plan: Some("max".to_owned()),
                sandbox: None,
            }
        );
        let signed_in = |sandbox| codex(sandbox, |fields| fields.signed_in = true);
        let not_configured = signed_in(CodexSandboxReadiness::NotConfigured);
        assert_eq!(
            sandbox_of(&not_configured, QUIET),
            Some(SandboxView::Needed)
        );
        let update_required = signed_in(CodexSandboxReadiness::UpdateRequired);
        assert_eq!(
            sandbox_of(&update_required, QUIET),
            Some(SandboxView::Needed)
        );
        assert_eq!(
            sandbox_of(
                &not_configured,
                LocalState {
                    signing_in: false,
                    preparing: true
                }
            ),
            Some(SandboxView::Preparing)
        );
        assert_eq!(
            sandbox_of(&signed_in(CodexSandboxReadiness::Unavailable), QUIET),
            Some(SandboxView::Unavailable)
        );
        assert_eq!(
            sandbox_of(&signed_in(CodexSandboxReadiness::Ready), QUIET),
            Some(SandboxView::Ready)
        );
        assert_eq!(plan_label(Some("max")), Some("Max plan".to_owned()));
        assert_eq!(plan_label(None), None);
    }

    #[test]
    fn keeps_a_sign_in_going_through_checks_that_report_the_same_sign_in() {
        let signed_out = claude(|_| {});
        let newer_check = claude(|fields| fields.version = Some("2.1.281".to_owned()));
        assert!(same_sign_in(Some(&newer_check), Some(&signed_out)));
        let failed = failed_setup("The check failed");
        assert!(same_sign_in(failed.first(), Some(&signed_out)));
        let account = claude(|fields| {
            fields.signed_in = true;
            fields.account = Some("owner@example.com".to_owned());
        });
        assert!(!same_sign_in(Some(&account), Some(&signed_out)));
        let signed_in = |sandbox, account: &str| {
            codex(sandbox, |fields| {
                fields.signed_in = true;
                fields.account = Some(account.to_owned());
            })
        };
        let codex_in = signed_in(CodexSandboxReadiness::NotConfigured, "owner@example.com");
        let ready = signed_in(CodexSandboxReadiness::Ready, "owner@example.com");
        assert!(same_sign_in(Some(&ready), Some(&codex_in)));
        let other_account = signed_in(CodexSandboxReadiness::NotConfigured, "work@example.com");
        assert!(!same_sign_in(Some(&other_account), Some(&codex_in)));
    }

    #[test]
    fn shows_a_failed_setup_check_on_both_cards_with_check_again() {
        let entries = failed_setup("The check failed");
        let providers: Vec<Provider> = entries
            .iter()
            .map(|entry| match entry {
                ProviderSetup::Claude(_) => Provider::Claude,
                ProviderSetup::Codex(_) => Provider::Codex,
            })
            .collect();
        assert_eq!(providers, vec![Provider::Claude, Provider::Codex]);
        for entry in &entries {
            assert_eq!(
                card_view(Some(entry), QUIET),
                CardView::Problem {
                    reason: "The check failed".to_owned()
                }
            );
        }
    }

    /// Both providers' sign-ins, each begun on the failed report: Claude's attempt is "c1", Codex's "x1".
    fn both() -> (SignIns, Vec<ProviderSetup>) {
        let setup = failed_setup("x");
        let sign_ins = HashMap::from([
            (
                Provider::Claude,
                SignIn {
                    since: setup.first().cloned(),
                    attempt: "c1".to_owned(),
                },
            ),
            (
                Provider::Codex,
                SignIn {
                    since: setup.get(1).cloned(),
                    attempt: "x1".to_owned(),
                },
            ),
        ]);
        (sign_ins, setup)
    }

    #[test]
    fn keeps_both_providers_sign_ins_and_the_same_object_while_neither_ended() {
        let (both, setup) = both();
        assert!(matches!(
            live_sign_ins(&both, &[], Some(setup.as_slice())),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            live_sign_ins(&both, &["other".to_owned()], Some(setup.as_slice())),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn ends_only_the_provider_whose_attempt_ended() {
        let (both, setup) = both();
        let claude_only = HashMap::from([(Provider::Claude, both[&Provider::Claude].clone())]);
        let codex_only = HashMap::from([(Provider::Codex, both[&Provider::Codex].clone())]);
        assert_eq!(
            live_sign_ins(&both, &["x1".to_owned()], Some(setup.as_slice())).into_owned(),
            claude_only
        );
        assert_eq!(
            live_sign_ins(&both, &["c1".to_owned()], Some(setup.as_slice())).into_owned(),
            codex_only
        );
    }

    #[test]
    fn ends_only_the_provider_whose_report_changed() {
        let (both, setup) = both();
        let signed_in: Vec<ProviderSetup> = setup
            .iter()
            .map(|entry| match entry {
                ProviderSetup::Claude(_) => claude(|fields| {
                    fields.signed_in = true;
                    fields.account = Some("a@b.c".to_owned());
                }),
                other => other.clone(),
            })
            .collect();
        let codex_only = HashMap::from([(Provider::Codex, both[&Provider::Codex].clone())]);
        assert_eq!(
            live_sign_ins(&both, &[], Some(signed_in.as_slice())).into_owned(),
            codex_only
        );
    }

    #[test]
    fn an_empty_error_or_plan_is_no_problem_and_no_plan_label_as_in_javascript() {
        let empty_error = claude(|fields| fields.error = Some(String::new()));
        assert_eq!(card_view(Some(&empty_error), QUIET), CardView::SignedOut);
        assert_eq!(plan_label(Some("")), None);
    }
}
