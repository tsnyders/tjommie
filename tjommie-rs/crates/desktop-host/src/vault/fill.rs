//! Typing a saved login into a page's form, privately (`native-browser-login.ts`). While a private interaction runs, the
//! form's two fields are found in an isolated world of the page's main frame, the secret is written straight into them
//! (never through a tool, a log or the page's own scripts), and afterwards they are cleared and the clearing verified.
//!
//! The browser is trusted with a login only if the page is an ordinary HTTPS page: one main frame at exactly the origin the
//! login belongs to, no credentials in its address, no nested frames, one password field in one same-origin POST form.

use std::fmt;

use protocol::vault::LoginOrigin;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::storage::VaultSecret;
use crate::browser::cdp::{Method, Nothing, method};
use crate::browser::engine::TabHandle;

/// Why a login could not be filled, in the words a person reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FillError {
    /// The page is not one a login may be typed into.
    Unsafe,
    /// The fields could not be shown to be empty again.
    NotCleared,
    /// The interaction changed under the fill (control, privacy, the page): its words.
    Changed(String),
    /// The browser failed the step.
    Browser(String),
}

impl fmt::Display for FillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsafe => f.write_str(
                "This page cannot receive this login safely. Use an ordinary HTTPS login form in the main page.",
            ),
            Self::NotCleared => f.write_str(
                "Logins could not verify cleared fields. Close this private browser before resuming.",
            ),
            Self::Changed(words) | Self::Browser(words) => f.write_str(words),
        }
    }
}

impl std::error::Error for FillError {}

impl From<crate::browser::cdp::CdpError> for FillError {
    fn from(error: crate::browser::cdp::CdpError) -> Self {
        Self::Browser(error.to_string())
    }
}

/// The re-check made before and after every step: it fails once the interaction is no longer the one that began.
pub type Check<'a> = &'a dyn Fn() -> Result<(), FillError>;

/// The form's fields as the isolated world holds them: kept between the prepare, the fill and the clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginFields {
    pub frame_id: String,
    pub object_id: String,
    pub origin: String,
}

// ---- the page's side ---------------------------------------------------------------------------------------------

const FORM_CHECK: &str = r#"
function eligible() {
  if (document.querySelector('iframe,frame,object,embed')) return null;
  const passwords = [...document.querySelectorAll('input[type="password"]')];
  if (passwords.length !== 1) return null;
  const p = passwords[0], form = p.form;
  if (!form || p.autocomplete === 'new-password' || form.method.toLowerCase() !== 'post' || form.target && form.target !== '_self') return null;
  if (new URL(form.action, location.href).origin !== location.origin) return null;
  for (const submit of form.querySelectorAll('[formaction],[formmethod],[formtarget]')) {
    if (submit.formAction && new URL(submit.formAction, location.href).origin !== location.origin || submit.formMethod && submit.formMethod.toLowerCase() !== 'post' || submit.formTarget && submit.formTarget !== '_self') return null;
  }
  let candidates = [...form.elements].filter(n => n instanceof HTMLInputElement && ['text','email'].includes(n.type) && (n.autocomplete === 'username' || n.type === 'email'));
  if (!candidates.length) candidates = [...form.elements].filter(n => n instanceof HTMLInputElement && ['text','email'].includes(n.type));
  if (candidates.length !== 1) return null;
  const u = candidates[0];
  for (const n of [u,p]) {
    const r = n.getBoundingClientRect(), style = getComputedStyle(n);
    if (!n.isConnected || n.disabled || n.readOnly || r.width <= 0 || r.height <= 0 || style.display === 'none' || style.visibility !== 'visible') return null;
  }
  return {u,p,form,url:location.href};
}"#;

fn prepare_script() -> String {
    format!("(() => {{ {FORM_CHECK} return eligible(); }})()")
}

fn fill_script() -> String {
    format!(
        r#"function(username,password,origin) {{
  {FORM_CHECK}
  const now = eligible();
  if (location.origin !== origin || location.href !== this.url || !now || now.u !== this.u || now.p !== this.p || now.form !== this.form) return false;
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;
  set.call(this.u,username); set.call(this.p,password);
  for (const n of [this.u,this.p]) {{ n.dispatchEvent(new Event('input',{{bubbles:true}})); n.dispatchEvent(new Event('change',{{bubbles:true}})); }}
  return true;
}}"#
    )
}

const CLEAR_SCRIPT: &str = r#"function() {
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;
  for (const n of [this.u,this.p]) { set.call(n,''); if(n.isConnected) n.dispatchEvent(new Event('input',{bubbles:true})); }
  return this.u.value === '' && this.p.value === '';
}"#;

// ---- the DevTools calls ------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Frame {
    id: String,
    url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FrameTree {
    frame: Frame,
    child_frames: Vec<FrameTree>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FrameTreeReply {
    frame_tree: FrameTree,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolatedWorldParams {
    frame_id: String,
    world_name: &'static str,
    // CDP spells this parameter with the typo.
    #[serde(rename = "grantUniveralAccess")]
    grant_universal_access: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IsolatedWorldReply {
    execution_context_id: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateInContext {
    expression: String,
    context_id: i64,
    return_by_value: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Ignored {}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ObjectResult {
    object_id: Option<String>,
    subtype: Option<String>,
    value: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EvaluateReply {
    result: ObjectResult,
    exception_details: Option<Ignored>,
}

/// One value passed to the page function. It is wiped when it drops; the encoded command is not this module's to wipe.
/// It has no `Debug` on purpose: nothing can print it.
#[derive(Serialize)]
pub struct SecretValue {
    value: Zeroizing<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallOn {
    object_id: String,
    function_declaration: String,
    arguments: Vec<SecretValue>,
    return_by_value: bool,
    silent: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseObject {
    object_id: String,
}

method!(
    PageGetFrameTree,
    "Page.getFrameTree",
    crate::browser::cdp::NoParams,
    FrameTreeReply
);
method!(
    PageCreateIsolatedWorld,
    "Page.createIsolatedWorld",
    IsolatedWorldParams,
    IsolatedWorldReply
);
method!(
    RuntimeEvaluateInContext,
    "Runtime.evaluate",
    EvaluateInContext,
    EvaluateReply
);
method!(
    RuntimeCallOn,
    "Runtime.callFunctionOn",
    CallOn,
    EvaluateReply
);
method!(
    RuntimeReleaseObject,
    "Runtime.releaseObject",
    ReleaseObject,
    Nothing
);

// ---- the steps ---------------------------------------------------------------------------------------------------

/// The id of the main frame, if the page is one a login may be typed into at `origin`: a single frame, an HTTPS address at
/// exactly that origin, with no credentials in it.
pub fn check_main_frame(tree: &FrameTree, origin: &str) -> Result<String, FillError> {
    if !tree.child_frames.is_empty() || tree.frame.id.is_empty() || tree.frame.url.is_empty() {
        return Err(FillError::Unsafe);
    }
    let url = url::Url::parse(&tree.frame.url).map_err(|_| FillError::Unsafe)?;
    let exact = url.scheme() == "https"
        && url.origin().ascii_serialization() == origin
        && url.username().is_empty()
        && url.password().is_none();
    if exact {
        Ok(tree.frame.id.clone())
    } else {
        Err(FillError::Unsafe)
    }
}

async fn main_frame(tab: &TabHandle, origin: &str) -> Result<String, FillError> {
    LoginOrigin::new(origin).map_err(|_| FillError::Unsafe)?;
    let reply = tab
        .cdp::<PageGetFrameTree>(&crate::browser::cdp::NoParams {})
        .await?;
    check_main_frame(&reply.frame_tree, origin)
}

/// Finds the login form of the page's main frame at `origin` and keeps its two fields in an isolated world.
pub async fn prepare(
    tab: &TabHandle,
    origin: &str,
    check: Check<'_>,
) -> Result<LoginFields, FillError> {
    check()?;
    let frame_id = main_frame(tab, origin).await?;
    check()?;
    let fields = prepare_in(tab, &frame_id, origin, check).await?;
    // The page must still be the same frame: the fields were found in what may have been a document in transition.
    if main_frame(tab, origin).await? != frame_id {
        return Err(FillError::Unsafe);
    }
    check()?;
    Ok(fields)
}

/// The steps after the frame has been vouched for: the isolated world and the form check. Exposed for tests that run
/// the page's side against a page that is not HTTPS.
pub async fn prepare_in(
    tab: &TabHandle,
    frame_id: &str,
    origin: &str,
    check: Check<'_>,
) -> Result<LoginFields, FillError> {
    let world = tab
        .cdp::<PageCreateIsolatedWorld>(&IsolatedWorldParams {
            frame_id: frame_id.to_string(),
            world_name: "Tjommie private login",
            grant_universal_access: false,
        })
        .await?;
    check()?;
    let context_id = world.execution_context_id.ok_or(FillError::Unsafe)?;
    let found = tab
        .cdp::<RuntimeEvaluateInContext>(&EvaluateInContext {
            expression: prepare_script(),
            context_id,
            return_by_value: false,
        })
        .await?;
    check()?;
    match (
        found.exception_details,
        found.result.object_id,
        found.result.subtype.as_deref(),
    ) {
        (None, Some(object_id), subtype) if subtype != Some("null") => Ok(LoginFields {
            frame_id: frame_id.to_string(),
            object_id,
            origin: origin.to_string(),
        }),
        _ => Err(FillError::Unsafe),
    }
}

/// Writes the secret into the prepared fields, if the page is still the one they were found in.
pub async fn fill(
    tab: &TabHandle,
    fields: &LoginFields,
    secret: &VaultSecret,
    check: Check<'_>,
) -> Result<(), FillError> {
    check()?;
    if main_frame(tab, &fields.origin).await? != fields.frame_id {
        return Err(FillError::Unsafe);
    }
    check()?;
    call_fill(tab, fields, secret, check).await
}

/// The fill itself, without the frame check (see [`prepare_in`]).
pub async fn call_fill(
    tab: &TabHandle,
    fields: &LoginFields,
    secret: &VaultSecret,
    check: Check<'_>,
) -> Result<(), FillError> {
    let answer = tab
        .cdp::<RuntimeCallOn>(&CallOn {
            object_id: fields.object_id.clone(),
            function_declaration: fill_script(),
            arguments: vec![
                SecretValue {
                    value: Zeroizing::new(secret.username().to_string()),
                },
                SecretValue {
                    value: Zeroizing::new(secret.password().to_string()),
                },
                SecretValue {
                    value: Zeroizing::new(fields.origin.clone()),
                },
            ],
            return_by_value: true,
            silent: true,
        })
        .await?;
    check()?;
    if answer.exception_details.is_some() || answer.result.value != Some(true) {
        return Err(FillError::Unsafe);
    }
    Ok(())
}

/// Empties the fields and checks that they are empty, then lets go of them. Fails closed: if emptiness cannot be shown,
/// the person is told to close this private browser.
pub async fn clear(
    tab: &TabHandle,
    fields: &LoginFields,
    check: Check<'_>,
) -> Result<(), FillError> {
    check()?;
    let answer = tab
        .cdp::<RuntimeCallOn>(&CallOn {
            object_id: fields.object_id.clone(),
            function_declaration: CLEAR_SCRIPT.to_string(),
            arguments: Vec::new(),
            return_by_value: true,
            silent: true,
        })
        .await?;
    check()?;
    if answer.exception_details.is_some() || answer.result.value != Some(true) {
        return Err(FillError::NotCleared);
    }
    tab.cdp::<RuntimeReleaseObject>(&ReleaseObject {
        object_id: fields.object_id.clone(),
    })
    .await?;
    check()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(url: &str, children: usize) -> FrameTree {
        FrameTree {
            frame: Frame {
                id: "main".into(),
                url: url.into(),
            },
            child_frames: vec![FrameTree::default(); children],
        }
    }

    #[test]
    fn only_an_ordinary_https_page_at_the_exact_origin_is_trusted_with_a_login() {
        let origin = "https://example.test";
        assert_eq!(
            check_main_frame(&tree("https://example.test/login?next=%2F", 0), origin),
            Ok("main".to_string())
        );
        for (url, children) in [
            ("http://example.test/login", 0),
            ("https://other.test/login", 0),
            ("https://example.test:8443/login", 0),
            ("https://user:pass@example.test/login", 0),
            ("https://user@example.test/login", 0),
            ("https://example.test/login", 1),
            ("about:blank", 0),
            ("not a url", 0),
            ("", 0),
        ] {
            assert_eq!(
                check_main_frame(&tree(url, children), origin),
                Err(FillError::Unsafe),
                "{url} with {children} frames"
            );
        }
        let mut nameless = tree("https://example.test/", 0);
        nameless.frame.id.clear();
        assert_eq!(check_main_frame(&nameless, origin), Err(FillError::Unsafe));
    }

    #[test]
    fn a_secret_is_serialized_into_the_command_and_printed_nowhere() {
        let call = SecretValue {
            value: Zeroizing::new("hunter2".to_string()),
        };
        let encoded = serde_json::to_string(&call).unwrap_or_default();
        assert_eq!(encoded, r#"{"value":"hunter2"}"#);
        assert!(!FillError::Unsafe.to_string().contains("hunter2"));
        assert_eq!(
            FillError::NotCleared.to_string(),
            "Logins could not verify cleared fields. Close this private browser before resuming."
        );
    }

    #[test]
    fn the_page_scripts_are_the_ones_of_the_typescript() {
        assert!(prepare_script().contains("input[type=\"password\"]"));
        assert!(fill_script().contains("location.origin !== origin"));
        assert!(fill_script().contains("HTMLInputElement.prototype"));
        assert!(CLEAR_SCRIPT.contains("this.u.value === '' && this.p.value === ''"));
    }
}
