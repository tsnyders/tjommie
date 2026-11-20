//! A tool at work on a page: the typed DevTools calls, element lookup by ref or CSS target, the actionability checks
//! (visible, enabled, stable, not covered), and the input a tool sends (`native-browser-automation.ts`: `evaluate`,
//! `element`, `box`, `actionable`, `key`, `click`, `fill`, `select`). WebView2 has no `sendInputEvent`, so input is
//! injected through the DevTools protocol, as the engine does for the person's own.

use std::rc::Rc;
use std::time::{Duration, Instant};

use protocol::browser::{BrowserControl, KeyAction};
use serde::de::DeserializeOwned;

use super::args::Button;
use super::page::Page;
use super::protocol::{
    ArgValue, CallArgument, CallFunctionOn, CallFunctionOnParams, DomDescribeNode, DomGetBoxModel,
    DomGetNodeForLocation, DomResolveNode, Evaluate, EvaluateParams, EvaluateReply,
    NodeForLocationParams, ObjectIdParams, PageGetLayoutMetrics, Readiness, RemoteObject,
    RemoteObjectRef, ResolveNodeParams, Scalar,
};
use super::reply::{ToolError, bounded};
use super::session::{ACTION, Context};
use crate::browser::cdp::{
    DispatchMouseParams, InputDispatchKeyEvent, InputDispatchMouseEvent, InputInsertText,
    InsertTextParams, Method, MouseEventType, NoParams, PointerButton,
};
use crate::browser::engine::TabHandle;
use crate::browser::{BotIdentity, keys};

/// The group the page objects of a tool call are kept in, and released together.
pub const GROUP: &str = "tjommie-tools";

/// What a tool needs to know about an element before it sends input to it.
const PROBE: &str = r#"function(editable) {
  if (!this.isConnected) return {error:'The element detached; take a fresh snapshot.'};
  if (this.disabled || this.getAttribute('aria-disabled') === 'true' || this.closest('[inert]')) return {error:'The element is disabled.'};
  if (editable && (this.readOnly || !(this instanceof HTMLInputElement || this instanceof HTMLTextAreaElement || this.isContentEditable))) return {error:'The element is not editable.'};
  const style = getComputedStyle(this); const rect = this.getBoundingClientRect();
  if (style.visibility === 'hidden' || style.display === 'none' || !rect.width || !rect.height) return {error:'The element is not visible.'};
  this.scrollIntoView({block:'center',inline:'center',behavior:'instant'});
  return {ok:true};
}"#;
const CONTAINS: &str = "function(hit) { return this === hit || this.contains(hit) || (hit && hit.getRootNode && hit.getRootNode().host === this); }";
const FOCUS: &str = r#"function(clear) {
  this.focus(); if (!clear) return;
  if (this instanceof HTMLInputElement || this instanceof HTMLTextAreaElement) { this.select(); }
  else { const range = document.createRange(); range.selectNodeContents(this); const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range); }
}"#;
const SELECT: &str = r#"function(values, byLabel) {
  if (!(this instanceof HTMLSelectElement)) throw new Error('Target is not a native select element. Use click/type for a custom combobox.');
  const chosen = Array.from(this.options).filter(option => values.includes(byLabel ? option.label : option.value));
  if (chosen.length !== values.length || (!this.multiple && chosen.length !== 1) || chosen.some(option => option.disabled)) throw new Error('Requested options are missing, disabled or incompatible.');
  for (const option of this.options) option.selected = chosen.includes(option);
  this.dispatchEvent(new Event('input', {bubbles:true})); this.dispatchEvent(new Event('change', {bubbles:true}));
}"#;
const CHECKED: &str = "function() { if (!(this instanceof HTMLInputElement) || !['checkbox','radio'].includes(this.type)) throw new Error('Target is not a checkbox or radio input.'); return this.checked; }";

/// An element's box in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// An element ready for input: its page object and the point to send it to.
pub struct Actionable {
    pub object_id: String,
    pub x: f64,
    pub y: f64,
}

/// One tool's view of a page: the call's context, the tab (awake and parked for as long as the call's session lives)
/// and what is remembered of the tab between calls.
pub struct Run<'a> {
    pub ctx: &'a Context,
    pub tab: TabHandle,
    pub page: Rc<Page>,
    pub bot: &'a BotIdentity,
}

/// `Math.round`: halves go up, also below zero.
pub fn js_round(value: f64) -> i64 {
    (value + 0.5).floor() as i64
}

/// The one element a call names: a target, a ref, or both if they agree.
pub fn target_of<'t>(
    target: Option<&'t str>,
    reference: Option<&'t str>,
) -> Result<&'t str, ToolError> {
    if let (Some(target), Some(reference)) = (target, reference)
        && target != reference
    {
        return Err(ToolError::new(
            "Use one target or matching ref, not conflicting targets.",
        ));
    }
    target
        .or(reference)
        .ok_or_else(|| ToolError::new("An exact snapshot ref or unique CSS target is required."))
}

/// Whether `value` is shaped like a ref a snapshot hands out: `n`, eight hex digits, `_`, a number.
fn looks_like_ref(value: &str) -> bool {
    let Some(rest) = value.strip_prefix('n') else {
        return false;
    };
    let Some((document, number)) = rest.split_once('_') else {
        return false;
    };
    document.len() == 8
        && document
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
}

/// What a failed page script says, bounded: the thrown error's text, or the exception's.
pub fn remote<T>(reply: EvaluateReply<T>) -> Result<RemoteObject<T>, ToolError> {
    match reply.exception_details {
        None => Ok(reply.result),
        Some(details) => {
            let message = details
                .exception
                .and_then(|exception| exception.description)
                .filter(|text| !text.is_empty())
                .or_else(|| Some(details.text).filter(|text| !text.is_empty()))
                .unwrap_or_else(|| "Page JavaScript failed.".to_string());
            Err(ToolError(bounded(&message, 2000)))
        }
    }
}

pub fn by_value(value: ArgValue) -> CallArgument {
    CallArgument::Value { value }
}

/// A modifier name as the tools take it, as a DevTools bit (alt 1, control 2, meta 4, shift 8). Control-or-meta is
/// control on Windows.
pub fn modifier(name: &str) -> Result<u32, ToolError> {
    match name {
        "Alt" => Ok(1),
        "Control" | "ControlOrMeta" => Ok(2),
        "Meta" => Ok(4),
        "Shift" => Ok(8),
        _ => Err(ToolError::new("Unknown key modifier.")),
    }
}

fn pointer_button(button: Button) -> (PointerButton, u32) {
    match button {
        Button::Left => (PointerButton::Left, 1),
        Button::Right => (PointerButton::Right, 2),
        Button::Middle => (PointerButton::Middle, 4),
    }
}

pub fn mouse_event(kind: MouseEventType, x: f64, y: f64) -> DispatchMouseParams {
    DispatchMouseParams {
        kind,
        x,
        y,
        modifiers: 0,
        button: PointerButton::None,
        buttons: 0,
        click_count: 0,
        delta_x: None,
        delta_y: None,
    }
}

/// A left-button event of a drag: a press holds the button, a move keeps holding it, a release lets go.
pub fn left_button(kind: MouseEventType, x: f64, y: f64) -> DispatchMouseParams {
    DispatchMouseParams {
        button: PointerButton::Left,
        buttons: u32::from(kind != MouseEventType::MouseReleased),
        click_count: u32::from(kind != MouseEventType::MouseMoved),
        ..mouse_event(kind, x, y)
    }
}

impl Run<'_> {
    pub async fn cdp<M: Method>(&self, params: &M::Params) -> Result<M::Reply, ToolError> {
        self.ctx.cdp::<M>(&self.tab, params).await
    }

    /// Applies what the page reported since the last look to what the tool remembers of it.
    pub fn refresh(&self) -> Result<(), ToolError> {
        let view = self.ctx.engine.tool_view(&self.bot.id)?;
        self.page
            .drain(view.control == BrowserControl::Bot && !view.private_active);
        Ok(())
    }

    /// A page expression's value (awaited if a promise), by value; `None` for `undefined`.
    pub async fn eval<T: DeserializeOwned>(
        &self,
        expression: &str,
    ) -> Result<Option<T>, ToolError> {
        let reply = self
            .cdp::<Evaluate<T>>(&EvaluateParams {
                expression: expression.to_string(),
                await_promise: Some(true),
                return_by_value: Some(true),
                timeout: Some(u32::try_from(ACTION.as_millis()).unwrap_or(10_000)),
                user_gesture: Some(true),
                object_group: None,
            })
            .await?;
        Ok(remote(reply)?.value)
    }

    /// A page function called on `object_id` as `this`, with the value it returns.
    pub async fn call<T: DeserializeOwned>(
        &self,
        object_id: &str,
        declaration: &str,
        arguments: Vec<CallArgument>,
    ) -> Result<Option<T>, ToolError> {
        let reply = self
            .cdp::<CallFunctionOn<T>>(&CallFunctionOnParams {
                object_id: object_id.to_string(),
                function_declaration: declaration.to_string(),
                arguments,
                await_promise: Some(true),
                return_by_value: Some(true),
                user_gesture: Some(true),
            })
            .await?;
        Ok(remote(reply)?.value)
    }

    /// The page object of an element named by a ref of the last snapshot, or by a CSS target that matches exactly one.
    pub async fn element(&self, value: &str) -> Result<String, ToolError> {
        self.refresh()?;
        let backend = self.page.state.borrow().refs.get(value).copied();
        if backend.is_none() && looks_like_ref(value) {
            return Err(ToolError::new(
                "This element ref is stale or belongs to another tab. Take a fresh snapshot.",
            ));
        }
        let object = match backend {
            Some(backend_node_id) => {
                self.cdp::<DomResolveNode>(&ResolveNodeParams {
                    backend_node_id,
                    object_group: GROUP.to_string(),
                })
                .await?
                .object
            }
            None => {
                let quoted = serde_json::to_string(value).unwrap_or_default();
                let expression = format!(
                    "(() => {{ const nodes = document.querySelectorAll({quoted}); if (nodes.length !== 1) throw new Error('Target must match exactly one element; take a fresh snapshot.'); return nodes[0]; }})()"
                );
                let reply = self
                    .cdp::<Evaluate<Scalar>>(&EvaluateParams {
                        expression,
                        await_promise: None,
                        return_by_value: Some(false),
                        timeout: Some(u32::try_from(ACTION.as_millis()).unwrap_or(10_000)),
                        user_gesture: None,
                        object_group: Some(GROUP.to_string()),
                    })
                    .await?;
                RemoteObjectRef {
                    object_id: remote(reply)?.object_id,
                }
            }
        };
        object.object_id.ok_or_else(|| {
            ToolError::new("The element is no longer available. Take a fresh snapshot.")
        })
    }

    /// An element's box (the bounds of its content quad).
    pub async fn rect(&self, object_id: &str) -> Result<Rect, ToolError> {
        let model = self
            .cdp::<DomGetBoxModel>(&ObjectIdParams {
                object_id: object_id.to_string(),
            })
            .await?
            .model;
        let [x0, y0, x1, y1, x2, y2, x3, y3] = model.content.as_slice() else {
            return Err(ToolError::new("The element has no visible box."));
        };
        let xs = [*x0, *x1, *x2, *x3];
        let ys = [*y0, *y1, *y2, *y3];
        if xs.iter().chain(ys.iter()).any(|value| !value.is_finite()) {
            return Err(ToolError::new("The element has no visible box."));
        }
        let (left, right) = xs
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let (top, bottom) = ys
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        Ok(Rect {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }

    /// An element that is visible, enabled (and editable, if asked), standing still and not covered, scrolled into view,
    /// with the point to send input to. Retries for three seconds, then says what was wrong.
    pub async fn actionable(&self, value: &str, editable: bool) -> Result<Actionable, ToolError> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut message = "The element is not ready for input.".to_string();
        while Instant::now() < deadline {
            let object_id = self.element(value).await?;
            let readiness: Option<Readiness> = self
                .call(&object_id, PROBE, vec![by_value(ArgValue::Bool(editable))])
                .await?;
            let readiness = readiness.unwrap_or(Readiness {
                ok: None,
                error: None,
            });
            if readiness.ok == Some(true) {
                let first = self.rect(&object_id).await?;
                self.ctx.delay(30).await?;
                let last = self.rect(&object_id).await?;
                let moved = (first.x - last.x).abs()
                    + (first.y - last.y).abs()
                    + (first.width - last.width).abs()
                    + (first.height - last.height).abs();
                if moved < 1.0 {
                    let (x, y) = (last.x + last.width / 2.0, last.y + last.height / 2.0);
                    // `DOM.getNodeForLocation` takes a point of the page, not of the window: where the page is scrolled to
                    // is added (a point above that is out of sight and finds nothing).
                    let view = self
                        .cdp::<PageGetLayoutMetrics>(&NoParams {})
                        .await?
                        .css_visual_viewport;
                    if self
                        .is_topmost(&object_id, x + view.page_x, y + view.page_y)
                        .await?
                    {
                        return Ok(Actionable { object_id, x, y });
                    }
                    message = "Another element covers the target; input was not sent.".to_string();
                } else {
                    message = "The element is moving; input was not sent.".to_string();
                }
            } else if let Some(error) = readiness.error.filter(|error| !error.is_empty()) {
                message = error;
            }
            self.ctx.delay(70).await?;
        }
        Err(ToolError(message))
    }

    /// Whether the element (or something inside it) is what a click at the page point `x`, `y` would reach.
    async fn is_topmost(&self, object_id: &str, x: f64, y: f64) -> Result<bool, ToolError> {
        let hit = self
            .cdp::<DomGetNodeForLocation>(&NodeForLocationParams {
                x: js_round(x) as i32,
                y: js_round(y) as i32,
                include_user_agent_shadow_dom: true,
            })
            .await?;
        let Some(backend_node_id) = hit.backend_node_id else {
            return Ok(false);
        };
        let hit_object = self
            .cdp::<DomResolveNode>(&ResolveNodeParams {
                backend_node_id,
                object_group: GROUP.to_string(),
            })
            .await?
            .object
            .object_id;
        let Some(hit_object) = hit_object else {
            return Ok(false);
        };
        let reply = self
            .cdp::<CallFunctionOn<Scalar>>(&CallFunctionOnParams {
                object_id: object_id.to_string(),
                function_declaration: CONTAINS.to_string(),
                arguments: vec![CallArgument::Object {
                    object_id: hit_object,
                }],
                await_promise: None,
                return_by_value: Some(true),
                user_gesture: None,
            })
            .await?;
        Ok(remote(reply)?.value == Some(Scalar::Bool(true)))
    }

    /// The backend node an element stands for (to find it in the accessibility tree).
    pub async fn backend_node(&self, object_id: &str) -> Result<Option<i64>, ToolError> {
        Ok(self
            .cdp::<DomDescribeNode>(&ObjectIdParams {
                object_id: object_id.to_string(),
            })
            .await?
            .node
            .backend_node_id)
    }

    // ---- input -----------------------------------------------------------------------------------------------

    /// What an input command's failure means. Input that blocks the page on a dialog (an `alert` in a click handler)
    /// did arrive: the call that follows reports the dialog. Anything else is the tool's failure.
    fn input_done<T>(&self, result: Result<T, ToolError>) -> Result<(), ToolError> {
        match result {
            Ok(_) => Ok(()),
            Err(_) if self.ctx.dialog_open() && self.ctx.guard().is_ok() => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub async fn mouse(&self, event: DispatchMouseParams) -> Result<(), ToolError> {
        let result = self.cdp::<InputDispatchMouseEvent>(&event).await;
        self.input_done(result)
    }

    /// A press and release of `button` at `x`, `y`, `times` times over (a double click is two, counted). The release
    /// is sent however the press went, so no button stays held.
    pub async fn click_at(
        &self,
        x: f64,
        y: f64,
        button: Button,
        times: u32,
        modifiers: u32,
    ) -> Result<(), ToolError> {
        let (pointer, bit) = pointer_button(button);
        let (x, y) = (js_round(x) as f64, js_round(y) as f64);
        self.ctx.guard()?;
        self.mouse(DispatchMouseParams {
            modifiers,
            ..mouse_event(MouseEventType::MouseMoved, x, y)
        })
        .await?;
        for count in 1..=times {
            let pressed = self
                .mouse(DispatchMouseParams {
                    modifiers,
                    button: pointer,
                    buttons: bit,
                    click_count: count,
                    ..mouse_event(MouseEventType::MousePressed, x, y)
                })
                .await;
            self.release(DispatchMouseParams {
                modifiers,
                button: pointer,
                click_count: count,
                ..mouse_event(MouseEventType::MouseReleased, x, y)
            })
            .await;
            pressed?;
        }
        self.ctx.delay(40).await
    }

    /// An event sent in a `finally`: bounded, and its failure does not matter.
    pub async fn release(&self, event: DispatchMouseParams) {
        self.ctx
            .cleanup::<InputDispatchMouseEvent>(&self.tab, &event)
            .await;
    }

    pub async fn click(
        &self,
        value: &str,
        button: Button,
        times: u32,
        modifiers: u32,
    ) -> Result<(), ToolError> {
        let point = self.actionable(value, false).await?;
        self.click_at(point.x, point.y, button, times, modifiers)
            .await
    }

    /// Text typed into the focused element in one piece.
    pub async fn insert_text(&self, text: &str) -> Result<(), ToolError> {
        self.ctx.guard()?;
        self.cdp::<InputInsertText>(&InsertTextParams {
            text: text.to_string(),
        })
        .await?;
        self.ctx.guard()
    }

    /// One key or chord (`Enter`, `ArrowLeft`, `Control+A`, a lone `+`).
    pub async fn key(&self, value: &str) -> Result<(), ToolError> {
        self.ctx.guard()?;
        let mut parts: Vec<&str> = if value == "+" {
            vec!["+"]
        } else if let Some(head) = value.strip_suffix("++") {
            let mut parts: Vec<&str> = head.split('+').collect();
            parts.push("+");
            parts
        } else {
            value.split('+').collect()
        };
        let name = parts.pop().unwrap_or_default();
        let mut modifiers = 0;
        for part in &parts {
            modifiers |= modifier(part)?;
        }
        if name.is_empty() {
            return Err(ToolError::new(
                "Use a key or modifier combination such as Control+A.",
            ));
        }
        let name = if name == "Space" { " " } else { name };
        // One character that needs two UTF-16 units is text, not a key.
        if name.chars().count() == 1 && name.encode_utf16().count() > 1 && modifiers == 0 {
            return self.insert_text(name).await;
        }
        let down = self
            .cdp::<InputDispatchKeyEvent>(&keys::key_event(name, KeyAction::Down, modifiers))
            .await;
        self.ctx
            .cleanup::<InputDispatchKeyEvent>(
                &self.tab,
                &keys::key_event(name, KeyAction::Up, modifiers),
            )
            .await;
        self.input_done(down)
    }

    /// Fills an editable element, replacing what is there (`text` empty clears it), or types it key by key.
    pub async fn fill(&self, value: &str, text: &str, slowly: bool) -> Result<(), ToolError> {
        let element = self.actionable(value, true).await?;
        self.call::<Scalar>(
            &element.object_id,
            FOCUS,
            vec![by_value(ArgValue::Bool(!slowly))],
        )
        .await?;
        if slowly {
            for character in text.chars() {
                self.key(&character.to_string()).await?;
            }
        } else if text.is_empty() {
            self.key("Backspace").await?;
        } else {
            self.insert_text(text).await?;
        }
        self.ctx.guard()
    }

    /// Chooses existing options of a native select, by value or by label.
    pub async fn select(
        &self,
        value: &str,
        values: &[String],
        by_label: bool,
    ) -> Result<(), ToolError> {
        let element = self.actionable(value, false).await?;
        self.call::<Scalar>(
            &element.object_id,
            SELECT,
            vec![
                by_value(ArgValue::Texts(values.to_vec())),
                by_value(ArgValue::Bool(by_label)),
            ],
        )
        .await?;
        Ok(())
    }

    /// Sets a checkbox or radio to `checked` by clicking it, if it is not already.
    pub async fn check(&self, value: &str, checked: bool) -> Result<(), ToolError> {
        let element = self.actionable(value, false).await?;
        let current: Option<Scalar> = self.call(&element.object_id, CHECKED, Vec::new()).await?;
        if current != Some(Scalar::Bool(checked)) {
            self.click(value, Button::Left, 1, 0).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_are_recognized_by_their_shape() {
        for good in ["n0123abcd_1", "nffffffff_204"] {
            assert!(looks_like_ref(good), "{good}");
        }
        for bad in [
            "#save",
            "n0123abcd_",
            "n0123abc_1",
            "n0123abcd1",
            "x0123abcd_1",
            "N0123abcd_1",
            "n0123ABCD_1",
            "n0123abcd_1a",
            "",
        ] {
            assert!(!looks_like_ref(bad), "{bad}");
        }
    }

    #[test]
    fn a_target_and_a_ref_must_agree_and_one_must_be_given() {
        assert_eq!(target_of(Some("#a"), None).unwrap(), "#a");
        assert_eq!(target_of(None, Some("n0123abcd_1")).unwrap(), "n0123abcd_1");
        assert_eq!(target_of(Some("#a"), Some("#a")).unwrap(), "#a");
        assert_eq!(
            target_of(Some("#a"), Some("#b")).unwrap_err().0,
            "Use one target or matching ref, not conflicting targets."
        );
        assert_eq!(
            target_of(None, None).unwrap_err().0,
            "An exact snapshot ref or unique CSS target is required."
        );
    }

    #[test]
    fn javascript_rounding_sends_halves_up() {
        assert_eq!(
            [
                js_round(0.5),
                js_round(1.5),
                js_round(-0.5),
                js_round(-1.5),
                js_round(2.4)
            ],
            [1, 2, 0, -1, 2]
        );
    }

    #[test]
    fn modifiers_are_devtools_bits_and_unknown_ones_are_refused() {
        assert_eq!(
            ["Alt", "Control", "ControlOrMeta", "Meta", "Shift"]
                .map(|name| modifier(name).unwrap()),
            [1, 2, 2, 4, 8]
        );
        assert_eq!(modifier("Hyper").unwrap_err().0, "Unknown key modifier.");
    }

    #[test]
    fn a_failed_script_says_what_the_page_said_bounded() {
        let thrown: EvaluateReply<Scalar> = serde_json::from_str(
            r#"{"result":{},"exceptionDetails":{"text":"Uncaught","exception":{"description":"Error: boom\n    at <anonymous>"}}}"#,
        )
        .unwrap();
        assert!(remote(thrown).unwrap_err().0.starts_with("Error: boom"));
        let bare: EvaluateReply<Scalar> =
            serde_json::from_str(r#"{"result":{},"exceptionDetails":{"text":"Uncaught"}}"#)
                .unwrap();
        assert_eq!(remote(bare).unwrap_err().0, "Uncaught");
        let none: EvaluateReply<Scalar> =
            serde_json::from_str(r#"{"result":{},"exceptionDetails":{}}"#).unwrap();
        assert_eq!(remote(none).unwrap_err().0, "Page JavaScript failed.");
        let ok: EvaluateReply<Scalar> = serde_json::from_str(r#"{"result":{"value":4}}"#).unwrap();
        assert_eq!(remote(ok).unwrap().value, Some(Scalar::Number(4.0)));
    }
}
