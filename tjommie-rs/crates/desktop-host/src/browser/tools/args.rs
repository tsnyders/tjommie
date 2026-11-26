//! Typed arguments of the 26 browser tools: a port of the zod schemas in `native-browser-automation.ts`, the
//! cross-field rules of `validateNativeBrowserTool` and the `requireTarget` refine.
//!
//! `decode` is the wire boundary. Everything after it works on typed values.

use std::collections::BTreeMap;
use std::fmt::Display;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

/// The largest integer JavaScript holds exactly. zod's `int()` accepts nothing above it.
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// One decoded call to a browser tool.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolCall {
    Snapshot(SnapshotArgs),
    Click(ClickArgs),
    Hover(TargetArgs),
    Type(TypeArgs),
    SelectOption(SelectOptionArgs),
    FillForm(FillFormArgs),
    Drag(DragArgs),
    Drop(DropArgs),
    EmulateMedia(EmulateMediaArgs),
    TakeScreenshot(ScreenshotArgs),
    Resize(ResizeArgs),
    PressKey(PressKeyArgs),
    Evaluate(EvaluateArgs),
    Navigate(NavigateArgs),
    NavigateBack,
    NavigateForward,
    Reload,
    Close,
    Tabs(TabsArgs),
    HandleDialog(HandleDialogArgs),
    ConsoleMessages(ConsoleArgs),
    NetworkRequests(NetworkRequestsArgs),
    NetworkRequest(NetworkRequestArgs),
    FileUpload(FileUploadArgs),
    WaitFor(WaitForArgs),
    Find(FindArgs),
}

/// Reads an optional field that is not nullable. Absent is `None`; `null` is an error, as in zod.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Reads an optional field that is also nullable. Absent is `None`, `null` is `Some(None)` (a reset) and a value
/// is `Some(Some(value))`.
fn nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// `browser_snapshot`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapshotArgs {
    #[serde(default, deserialize_with = "present")]
    pub element: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub target: Option<String>,
    #[serde(default, rename = "ref", deserialize_with = "present")]
    pub reference: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub filename: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub depth: Option<u32>,
    #[serde(default, deserialize_with = "present")]
    pub boxes: Option<bool>,
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// A modifier key held during a click.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Modifier {
    Alt,
    Control,
    ControlOrMeta,
    Meta,
    Shift,
}

/// `browser_click`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ClickArgs {
    #[serde(default, deserialize_with = "present")]
    pub element: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub target: Option<String>,
    #[serde(default, rename = "ref", deserialize_with = "present")]
    pub reference: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub double_click: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    pub button: Option<Button>,
    #[serde(default, deserialize_with = "present")]
    pub modifiers: Option<Vec<Modifier>>,
}

/// `browser_hover`: an element and nothing else.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TargetArgs {
    #[serde(default, deserialize_with = "present")]
    pub element: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub target: Option<String>,
    #[serde(default, rename = "ref", deserialize_with = "present")]
    pub reference: Option<String>,
}

/// `browser_type`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TypeArgs {
    #[serde(default, deserialize_with = "present")]
    pub element: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub target: Option<String>,
    #[serde(default, rename = "ref", deserialize_with = "present")]
    pub reference: Option<String>,
    pub text: String,
    #[serde(default, deserialize_with = "present")]
    pub slowly: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    pub submit: Option<bool>,
}

/// `browser_select_option`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SelectOptionArgs {
    #[serde(default, deserialize_with = "present")]
    pub element: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub target: Option<String>,
    #[serde(default, rename = "ref", deserialize_with = "present")]
    pub reference: Option<String>,
    pub values: Vec<String>,
}

/// The kind of a `browser_fill_form` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Textbox,
    Checkbox,
    Radio,
    Combobox,
    Slider,
}

/// One field of `browser_fill_form`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FormField {
    #[serde(default, deserialize_with = "present")]
    pub element: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub target: Option<String>,
    #[serde(default, rename = "ref", deserialize_with = "present")]
    pub reference: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    pub value: String,
}

/// `browser_fill_form`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FillFormArgs {
    pub fields: Vec<FormField>,
}

/// `browser_drag`. Start and end each take a target or a ref, and both are optional in the schema.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DragArgs {
    #[serde(default, deserialize_with = "present")]
    pub start_element: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub start_target: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub start_ref: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub end_element: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub end_target: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub end_ref: Option<String>,
}

/// `browser_drop`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DropArgs {
    #[serde(default, deserialize_with = "present")]
    pub element: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub target: Option<String>,
    #[serde(default, rename = "ref", deserialize_with = "present")]
    pub reference: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub paths: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    pub data: Option<BTreeMap<String, String>>,
}

/// The colour scheme the page reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColorScheme {
    Light,
    Dark,
    NoPreference,
}

/// The reduced-motion preference the page reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReducedMotion {
    Reduce,
    NoPreference,
}

/// The forced-colors setting the page reports. `Off` is the wire value `none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum ForcedColors {
    Active,
    #[serde(rename = "none")]
    Off,
}

/// The contrast preference the page reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Contrast {
    More,
    NoPreference,
}

/// The CSS media type the page reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Media {
    Screen,
    Print,
}

/// `browser_emulate_media`. Each field is absent (leave it), `null` (reset it) or a value.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EmulateMediaArgs {
    #[serde(default, deserialize_with = "nullable")]
    pub color_scheme: Option<Option<ColorScheme>>,
    #[serde(default, deserialize_with = "nullable")]
    pub reduced_motion: Option<Option<ReducedMotion>>,
    #[serde(default, deserialize_with = "nullable")]
    pub forced_colors: Option<Option<ForcedColors>>,
    #[serde(default, deserialize_with = "nullable")]
    pub contrast: Option<Option<Contrast>>,
    #[serde(default, deserialize_with = "nullable")]
    pub media: Option<Option<Media>>,
}

/// The image format of a screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageType {
    Png,
    Jpeg,
    Webp,
}

/// Whether a screenshot uses CSS pixels or device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scale {
    Css,
    Device,
}

/// `browser_take_screenshot`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScreenshotArgs {
    #[serde(default, deserialize_with = "present")]
    pub element: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub target: Option<String>,
    #[serde(default, rename = "ref", deserialize_with = "present")]
    pub reference: Option<String>,
    #[serde(default, rename = "type", deserialize_with = "present")]
    pub image_type: Option<ImageType>,
    #[serde(default, deserialize_with = "present")]
    pub filename: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub full_page: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    pub scale: Option<Scale>,
}

/// `browser_resize`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResizeArgs {
    pub width: u32,
    pub height: u32,
}

/// `browser_press_key`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PressKeyArgs {
    pub key: String,
}

/// `browser_evaluate`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EvaluateArgs {
    #[serde(default, deserialize_with = "present")]
    pub element: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub target: Option<String>,
    #[serde(default, rename = "ref", deserialize_with = "present")]
    pub reference: Option<String>,
    pub function: String,
    #[serde(default, deserialize_with = "present")]
    pub filename: Option<String>,
}

/// `browser_navigate`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NavigateArgs {
    pub url: String,
}

/// The action of `browser_tabs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabAction {
    List,
    New,
    Close,
    Select,
}

/// `browser_tabs`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TabsArgs {
    pub action: TabAction,
    #[serde(default, deserialize_with = "present")]
    pub index: Option<u32>,
    #[serde(default, deserialize_with = "present")]
    pub url: Option<String>,
}

/// `browser_handle_dialog`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HandleDialogArgs {
    pub accept: bool,
    #[serde(default, deserialize_with = "present")]
    pub prompt_text: Option<String>,
}

/// The lowest console level a `browser_console_messages` call shows. Defaults to `info`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleLevel {
    Error,
    Warning,
    #[default]
    Info,
    Debug,
}

/// `browser_console_messages`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConsoleArgs {
    #[serde(default)]
    pub level: ConsoleLevel,
    #[serde(default, deserialize_with = "present")]
    pub all: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    pub filename: Option<String>,
}

/// `browser_network_requests`. `static` defaults to false.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NetworkRequestsArgs {
    #[serde(default, rename = "static")]
    pub static_resources: bool,
    #[serde(default, deserialize_with = "present")]
    pub include_static: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    pub filter: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub filename: Option<String>,
}

/// The part of a request `browser_network_request` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequestPart {
    RequestHeaders,
    RequestBody,
    ResponseHeaders,
    ResponseBody,
}

/// `browser_network_request`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NetworkRequestArgs {
    pub index: u64,
    #[serde(default, deserialize_with = "present")]
    pub part: Option<RequestPart>,
    #[serde(default, deserialize_with = "present")]
    pub filename: Option<String>,
}

/// `browser_file_upload`. Omitting `paths` cancels the pending chooser.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileUploadArgs {
    #[serde(default, deserialize_with = "present")]
    pub paths: Option<Vec<String>>,
}

/// `browser_wait_for`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WaitForArgs {
    #[serde(default, deserialize_with = "present")]
    pub time: Option<f64>,
    #[serde(default, deserialize_with = "present")]
    pub text: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub text_gone: Option<String>,
}

/// `browser_find`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FindArgs {
    #[serde(default, deserialize_with = "present")]
    pub text: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub regex: Option<String>,
}

/// The empty argument object of the tools that take none. Any key is an error.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

impl ToolCall {
    /// The tool's wire name, such as `browser_click`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Snapshot(_) => "browser_snapshot",
            Self::Click(_) => "browser_click",
            Self::Hover(_) => "browser_hover",
            Self::Type(_) => "browser_type",
            Self::SelectOption(_) => "browser_select_option",
            Self::FillForm(_) => "browser_fill_form",
            Self::Drag(_) => "browser_drag",
            Self::Drop(_) => "browser_drop",
            Self::EmulateMedia(_) => "browser_emulate_media",
            Self::TakeScreenshot(_) => "browser_take_screenshot",
            Self::Resize(_) => "browser_resize",
            Self::PressKey(_) => "browser_press_key",
            Self::Evaluate(_) => "browser_evaluate",
            Self::Navigate(_) => "browser_navigate",
            Self::NavigateBack => "browser_navigate_back",
            Self::NavigateForward => "browser_navigate_forward",
            Self::Reload => "browser_reload",
            Self::Close => "browser_close",
            Self::Tabs(_) => "browser_tabs",
            Self::HandleDialog(_) => "browser_handle_dialog",
            Self::ConsoleMessages(_) => "browser_console_messages",
            Self::NetworkRequests(_) => "browser_network_requests",
            Self::NetworkRequest(_) => "browser_network_request",
            Self::FileUpload(_) => "browser_file_upload",
            Self::WaitFor(_) => "browser_wait_for",
            Self::Find(_) => "browser_find",
        }
    }

    /// Whether the tool works on the shared page. The other eight run without one.
    pub fn needs_page(&self) -> bool {
        !matches!(
            self,
            Self::Resize(_)
                | Self::Navigate(_)
                | Self::NavigateBack
                | Self::NavigateForward
                | Self::Reload
                | Self::Close
                | Self::Tabs(_)
                | Self::HandleDialog(_)
        )
    }
}

/// Decodes one tool call from its wire name and arguments, as `validateNativeBrowserTool` does.
///
/// The shape is decoded first, then the zod limits, then the cross-field rules. Errors are the desktop's
/// messages: schema failures carry the tool name, and the cross-field rules are plain.
pub fn decode(name: &str, args: &Map<String, Value>) -> Result<ToolCall, String> {
    let value = Value::Object(args.clone());
    let call = match name {
        "browser_snapshot" => ToolCall::Snapshot(typed(name, value)?),
        "browser_click" => ToolCall::Click(typed(name, value)?),
        "browser_hover" => ToolCall::Hover(typed(name, value)?),
        "browser_type" => ToolCall::Type(typed(name, value)?),
        "browser_select_option" => ToolCall::SelectOption(typed(name, value)?),
        "browser_fill_form" => ToolCall::FillForm(typed(name, value)?),
        "browser_drag" => ToolCall::Drag(typed(name, value)?),
        "browser_drop" => ToolCall::Drop(typed(name, value)?),
        "browser_emulate_media" => ToolCall::EmulateMedia(typed(name, value)?),
        "browser_take_screenshot" => ToolCall::TakeScreenshot(typed(name, value)?),
        "browser_resize" => ToolCall::Resize(typed(name, value)?),
        "browser_press_key" => ToolCall::PressKey(typed(name, value)?),
        "browser_evaluate" => ToolCall::Evaluate(typed(name, value)?),
        "browser_navigate" => ToolCall::Navigate(typed(name, value)?),
        "browser_navigate_back" => {
            typed::<NoArgs>(name, value)?;
            ToolCall::NavigateBack
        }
        "browser_navigate_forward" => {
            typed::<NoArgs>(name, value)?;
            ToolCall::NavigateForward
        }
        "browser_reload" => {
            typed::<NoArgs>(name, value)?;
            ToolCall::Reload
        }
        "browser_close" => {
            typed::<NoArgs>(name, value)?;
            ToolCall::Close
        }
        "browser_tabs" => ToolCall::Tabs(typed(name, value)?),
        "browser_handle_dialog" => ToolCall::HandleDialog(typed(name, value)?),
        "browser_console_messages" => ToolCall::ConsoleMessages(typed(name, value)?),
        "browser_network_requests" => ToolCall::NetworkRequests(typed(name, value)?),
        "browser_network_request" => ToolCall::NetworkRequest(typed(name, value)?),
        "browser_file_upload" => ToolCall::FileUpload(typed(name, value)?),
        "browser_wait_for" => ToolCall::WaitFor(typed(name, value)?),
        "browser_find" => ToolCall::Find(typed(name, value)?),
        _ => {
            return Err(format!(
                "Unsupported native browser tool: {}.",
                name.chars().take(100).collect::<String>()
            ));
        }
    };
    rules(&call).map_err(String::from)?;
    Ok(call)
}

/// Decodes the arguments into a typed struct, then checks its zod limits.
fn typed<T: DeserializeOwned + Limits>(name: &str, value: Value) -> Result<T, String> {
    let parsed: T = serde_json::from_value(value)
        .map_err(|error| format!("Invalid arguments for {name}: arguments: {error}"))?;
    parsed
        .limits()
        .map_err(|issue| format!("Invalid arguments for {name}: {issue}"))?;
    Ok(parsed)
}

/// The limits zod puts on a tool's fields. Each failure is `path: message`, as zod 4 words it.
trait Limits {
    fn limits(&self) -> Result<(), String>;
}

/// A string's length, counted in UTF-16 units as JavaScript counts it.
fn text(path: &str, value: &str, min: usize, max: usize) -> Result<(), String> {
    let units = value.encode_utf16().count();
    if units < min {
        Err(format!(
            "{path}: Too small: expected string to have >={min} characters"
        ))
    } else if units > max {
        Err(format!(
            "{path}: Too big: expected string to have <={max} characters"
        ))
    } else {
        Ok(())
    }
}

/// An optional string's length, when it is present.
fn optional_text(path: &str, value: &Option<String>, min: usize, max: usize) -> Result<(), String> {
    value
        .as_deref()
        .map_or(Ok(()), |value| text(path, value, min, max))
}

/// Each string of a list, with its index in the path.
fn each_text(path: &str, values: &[String], min: usize, max: usize) -> Result<(), String> {
    values
        .iter()
        .enumerate()
        .try_for_each(|(index, value)| text(&format!("{path}.{index}"), value, min, max))
}

/// A list's length.
fn items(path: &str, len: usize, min: usize, max: usize) -> Result<(), String> {
    if len < min {
        Err(format!(
            "{path}: Too small: expected array to have >={min} items"
        ))
    } else if len > max {
        Err(format!(
            "{path}: Too big: expected array to have <={max} items"
        ))
    } else {
        Ok(())
    }
}

/// A number within an inclusive range.
fn range<T: PartialOrd + Display>(path: &str, value: T, min: T, max: T) -> Result<(), String> {
    if value < min {
        Err(format!("{path}: Too small: expected number to be >={min}"))
    } else if value > max {
        Err(format!("{path}: Too big: expected number to be <={max}"))
    } else {
        Ok(())
    }
}

/// The element, target and ref limits of the tools that act on an element.
fn element_limits(
    element: &Option<String>,
    target: &Option<String>,
    reference: &Option<String>,
) -> Result<(), String> {
    optional_text("element", element, 0, 1000)?;
    optional_text("target", target, 1, 2000)?;
    optional_text("ref", reference, 1, 2000)
}

/// The desktop's `target()` rule: one target or ref, and the two must match when both are given.
fn check_target(target: Option<&str>, reference: Option<&str>) -> Result<(), &'static str> {
    match (target, reference) {
        (Some(a), Some(b)) if a != b => {
            Err("Use one target or matching ref, not conflicting targets.")
        }
        (Some(_), _) | (None, Some(_)) => Ok(()),
        (None, None) => Err("An exact snapshot ref or unique CSS target is required."),
    }
}

/// An optional string with the empty string taken as absent, as the desktop reads it.
fn given(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

/// The cross-field rules of `validateNativeBrowserTool`, with the desktop's messages.
fn rules(call: &ToolCall) -> Result<(), &'static str> {
    match call {
        ToolCall::Click(args) => check_target(given(&args.target), given(&args.reference)),
        ToolCall::Hover(args) => check_target(given(&args.target), given(&args.reference)),
        ToolCall::Type(args) => check_target(given(&args.target), given(&args.reference)),
        ToolCall::SelectOption(args) => check_target(given(&args.target), given(&args.reference)),
        ToolCall::Drop(args) => check_target(given(&args.target), given(&args.reference)),
        ToolCall::Drag(args) => {
            check_target(given(&args.start_target), given(&args.start_ref))?;
            check_target(given(&args.end_target), given(&args.end_ref))
        }
        ToolCall::Find(args) if args.text.is_some() == args.regex.is_some() => {
            Err("Supply text or regex, not both.")
        }
        ToolCall::WaitFor(args)
            if args.time.is_none() && args.text.is_none() && args.text_gone.is_none() =>
        {
            Err("Supply time, text or textGone.")
        }
        ToolCall::Tabs(args) if args.action != TabAction::New && given(&args.url).is_some() => {
            Err("A URL is supported only when creating a new tab.")
        }
        _ => Ok(()),
    }
}

impl Limits for NoArgs {
    fn limits(&self) -> Result<(), String> {
        Ok(())
    }
}

impl Limits for SnapshotArgs {
    fn limits(&self) -> Result<(), String> {
        element_limits(&self.element, &self.target, &self.reference)?;
        optional_text("filename", &self.filename, 1, 255)?;
        self.depth
            .map_or(Ok(()), |depth| range("depth", depth, 0, 30))
    }
}

impl Limits for ClickArgs {
    fn limits(&self) -> Result<(), String> {
        element_limits(&self.element, &self.target, &self.reference)?;
        self.modifiers.as_ref().map_or(Ok(()), |modifiers| {
            items("modifiers", modifiers.len(), 0, 5)
        })
    }
}

impl Limits for TargetArgs {
    fn limits(&self) -> Result<(), String> {
        element_limits(&self.element, &self.target, &self.reference)
    }
}

impl Limits for TypeArgs {
    fn limits(&self) -> Result<(), String> {
        element_limits(&self.element, &self.target, &self.reference)?;
        text("text", &self.text, 0, 100_000)
    }
}

impl Limits for SelectOptionArgs {
    fn limits(&self) -> Result<(), String> {
        element_limits(&self.element, &self.target, &self.reference)?;
        items("values", self.values.len(), 1, 100)?;
        each_text("values", &self.values, 0, 2000)
    }
}

impl Limits for FormField {
    fn limits(&self) -> Result<(), String> {
        element_limits(&self.element, &self.target, &self.reference)?;
        text("name", &self.name, 0, 1000)?;
        text("value", &self.value, 0, 100_000)
    }
}

impl Limits for FillFormArgs {
    fn limits(&self) -> Result<(), String> {
        items("fields", self.fields.len(), 1, 30)?;
        for (index, field) in self.fields.iter().enumerate() {
            field
                .limits()
                .map_err(|issue| format!("fields.{index}.{issue}"))?;
            // The `requireTarget` refine: the field needs one target or ref, and both must match.
            if check_target(given(&field.target), given(&field.reference)).is_err() {
                return Err(format!(
                    "fields.{index}: An exact target or ref is required."
                ));
            }
        }
        Ok(())
    }
}

impl Limits for DragArgs {
    fn limits(&self) -> Result<(), String> {
        optional_text("startElement", &self.start_element, 0, 1000)?;
        optional_text("startTarget", &self.start_target, 0, 2000)?;
        optional_text("startRef", &self.start_ref, 0, 2000)?;
        optional_text("endElement", &self.end_element, 0, 1000)?;
        optional_text("endTarget", &self.end_target, 0, 2000)?;
        optional_text("endRef", &self.end_ref, 0, 2000)
    }
}

impl Limits for DropArgs {
    fn limits(&self) -> Result<(), String> {
        element_limits(&self.element, &self.target, &self.reference)?;
        if let Some(paths) = &self.paths {
            items("paths", paths.len(), 1, 20)?;
            each_text("paths", paths, 0, 4096)?;
        }
        if let Some(data) = &self.data {
            for (mime_type, value) in data {
                text("data", mime_type, 1, 100)?;
                text(&format!("data.{mime_type}"), value, 0, 100_000)?;
            }
        }
        Ok(())
    }
}

impl Limits for EmulateMediaArgs {
    fn limits(&self) -> Result<(), String> {
        Ok(())
    }
}

impl Limits for ScreenshotArgs {
    fn limits(&self) -> Result<(), String> {
        element_limits(&self.element, &self.target, &self.reference)?;
        optional_text("filename", &self.filename, 1, 255)
    }
}

impl Limits for ResizeArgs {
    fn limits(&self) -> Result<(), String> {
        range("width", self.width, 240, 2560)?;
        range("height", self.height, 180, 1600)
    }
}

impl Limits for PressKeyArgs {
    fn limits(&self) -> Result<(), String> {
        text("key", &self.key, 1, 100)
    }
}

impl Limits for EvaluateArgs {
    fn limits(&self) -> Result<(), String> {
        element_limits(&self.element, &self.target, &self.reference)?;
        text("function", &self.function, 1, 50_000)?;
        optional_text("filename", &self.filename, 1, 255)
    }
}

impl Limits for NavigateArgs {
    fn limits(&self) -> Result<(), String> {
        text("url", &self.url, 1, 8192)
    }
}

impl Limits for TabsArgs {
    fn limits(&self) -> Result<(), String> {
        self.index
            .map_or(Ok(()), |index| range("index", index, 0, 99))?;
        optional_text("url", &self.url, 0, 8192)
    }
}

impl Limits for HandleDialogArgs {
    fn limits(&self) -> Result<(), String> {
        optional_text("promptText", &self.prompt_text, 0, 10_000)
    }
}

impl Limits for ConsoleArgs {
    fn limits(&self) -> Result<(), String> {
        optional_text("filename", &self.filename, 1, 255)
    }
}

impl Limits for NetworkRequestsArgs {
    fn limits(&self) -> Result<(), String> {
        optional_text("filter", &self.filter, 0, 128)?;
        optional_text("filename", &self.filename, 1, 255)
    }
}

impl Limits for NetworkRequestArgs {
    fn limits(&self) -> Result<(), String> {
        range("index", self.index, 1, MAX_SAFE_INTEGER)?;
        optional_text("filename", &self.filename, 1, 255)
    }
}

impl Limits for FileUploadArgs {
    fn limits(&self) -> Result<(), String> {
        if let Some(paths) = &self.paths {
            items("paths", paths.len(), 0, 20)?;
            each_text("paths", paths, 1, 4096)?;
        }
        Ok(())
    }
}

impl Limits for WaitForArgs {
    fn limits(&self) -> Result<(), String> {
        self.time
            .map_or(Ok(()), |time| range("time", time, 0.0, 20.0))?;
        optional_text("text", &self.text, 1, 2000)?;
        optional_text("textGone", &self.text_gone, 1, 2000)
    }
}

impl Limits for FindArgs {
    fn limits(&self) -> Result<(), String> {
        optional_text("text", &self.text, 1, 2000)?;
        optional_text("regex", &self.regex, 1, 128)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, args: Value) -> Result<ToolCall, String> {
        decode(
            name,
            args.as_object().expect("test arguments are an object"),
        )
    }

    fn error(name: &str, args: Value) -> String {
        call(name, args).expect_err("the call should be rejected")
    }

    /// The 26 tools, each with the smallest valid arguments.
    fn minimal() -> Vec<(&'static str, Value)> {
        vec![
            ("browser_snapshot", json!({})),
            ("browser_click", json!({ "target": "#save" })),
            ("browser_hover", json!({ "target": "#save" })),
            ("browser_type", json!({ "target": "#name", "text": "Ada" })),
            (
                "browser_select_option",
                json!({ "target": "#pick", "values": ["one"] }),
            ),
            (
                "browser_fill_form",
                json!({ "fields": [{ "target": "#name", "name": "Name", "type": "textbox", "value": "Ada" }] }),
            ),
            (
                "browser_drag",
                json!({ "startTarget": "#a", "endTarget": "#b" }),
            ),
            (
                "browser_drop",
                json!({ "target": "#zone", "paths": ["a.txt"] }),
            ),
            ("browser_emulate_media", json!({})),
            ("browser_take_screenshot", json!({})),
            ("browser_resize", json!({ "width": 800, "height": 600 })),
            ("browser_press_key", json!({ "key": "Enter" })),
            ("browser_evaluate", json!({ "function": "() => 1" })),
            ("browser_navigate", json!({ "url": "https://example.test" })),
            ("browser_navigate_back", json!({})),
            ("browser_navigate_forward", json!({})),
            ("browser_reload", json!({})),
            ("browser_close", json!({})),
            ("browser_tabs", json!({ "action": "list" })),
            ("browser_handle_dialog", json!({ "accept": true })),
            ("browser_console_messages", json!({})),
            ("browser_network_requests", json!({})),
            ("browser_network_request", json!({ "index": 1 })),
            ("browser_file_upload", json!({})),
            ("browser_wait_for", json!({ "time": 1 })),
            ("browser_find", json!({ "text": "Save" })),
        ]
    }

    #[test]
    fn ports_the_first_desktop_test() {
        assert_eq!(
            error("browser_run_code", json!({})),
            "Unsupported native browser tool: browser_run_code."
        );
        assert_eq!(
            error("browser_run_code_unsafe", json!({})),
            "Unsupported native browser tool: browser_run_code_unsafe."
        );
        assert!(
            error("browser_click", json!({ "ref": "ref", "shell": "bad" }))
                .starts_with("Invalid arguments for browser_click")
        );
        assert!(
            error("browser_wait_for", json!({ "time": 60 }))
                .starts_with("Invalid arguments for browser_wait_for")
        );
        assert_eq!(
            error("__proto__", json!({})),
            "Unsupported native browser tool: __proto__."
        );
        assert!(
            error("browser_close", json!({ "shell": "bad" }))
                .starts_with("Invalid arguments for browser_close")
        );
    }

    #[test]
    fn all_26_tools_decode_and_name_themselves() {
        let cases = minimal();
        assert_eq!(cases.len(), 26);
        for (name, args) in cases {
            let decoded = call(name, args).unwrap_or_else(|reason| panic!("{name}: {reason}"));
            assert_eq!(decoded.name(), name);
        }
    }

    #[test]
    fn only_eight_tools_skip_the_page() {
        let tabless = [
            "browser_resize",
            "browser_navigate",
            "browser_navigate_back",
            "browser_navigate_forward",
            "browser_reload",
            "browser_close",
            "browser_tabs",
            "browser_handle_dialog",
        ];
        for (name, args) in minimal() {
            let decoded = call(name, args).expect("minimal arguments decode");
            assert_eq!(decoded.needs_page(), !tabless.contains(&name), "{name}");
        }
    }

    #[test]
    fn snapshot_depth_is_at_most_30() {
        assert!(call("browser_snapshot", json!({ "depth": 30 })).is_ok());
        assert_eq!(
            error("browser_snapshot", json!({ "depth": 31 })),
            "Invalid arguments for browser_snapshot: depth: Too big: expected number to be <=30"
        );
    }

    #[test]
    fn drop_takes_at_most_20_paths() {
        assert!(
            call(
                "browser_drop",
                json!({ "target": "#zone", "paths": vec!["a.txt"; 20] })
            )
            .is_ok()
        );
        assert_eq!(
            error(
                "browser_drop",
                json!({ "target": "#zone", "paths": vec!["a.txt"; 21] })
            ),
            "Invalid arguments for browser_drop: paths: Too big: expected array to have <=20 items"
        );
    }

    #[test]
    fn fill_form_takes_1_to_30_fields() {
        let field = json!({ "target": "#name", "name": "Name", "type": "textbox", "value": "Ada" });
        assert!(
            call(
                "browser_fill_form",
                json!({ "fields": vec![field.clone(); 30] })
            )
            .is_ok()
        );
        assert_eq!(
            error(
                "browser_fill_form",
                json!({ "fields": Vec::<Value>::new() })
            ),
            "Invalid arguments for browser_fill_form: fields: Too small: expected array to have >=1 items"
        );
        assert_eq!(
            error("browser_fill_form", json!({ "fields": vec![field; 31] })),
            "Invalid arguments for browser_fill_form: fields: Too big: expected array to have <=30 items"
        );
    }

    #[test]
    fn fill_form_fields_need_one_exact_target() {
        let none = json!({ "name": "Name", "type": "textbox", "value": "Ada" });
        let both = json!({ "target": "#a", "ref": "n1", "name": "Name", "type": "textbox", "value": "Ada" });
        assert_eq!(
            error("browser_fill_form", json!({ "fields": [none] })),
            "Invalid arguments for browser_fill_form: fields.0: An exact target or ref is required."
        );
        assert_eq!(
            error("browser_fill_form", json!({ "fields": [both] })),
            "Invalid arguments for browser_fill_form: fields.0: An exact target or ref is required."
        );
    }

    #[test]
    fn tabs_url_is_only_for_a_new_tab() {
        assert_eq!(
            error(
                "browser_tabs",
                json!({ "action": "list", "url": "https://example.test" })
            ),
            "A URL is supported only when creating a new tab."
        );
        assert!(
            call(
                "browser_tabs",
                json!({ "action": "new", "url": "https://example.test" })
            )
            .is_ok()
        );
        // zod allows an empty url, and the desktop treats an empty string as no url.
        assert!(call("browser_tabs", json!({ "action": "list", "url": "" })).is_ok());
    }

    #[test]
    fn find_takes_text_or_regex_but_not_both() {
        assert_eq!(
            error("browser_find", json!({ "text": "Save", "regex": "S.e" })),
            "Supply text or regex, not both."
        );
        assert_eq!(
            error("browser_find", json!({})),
            "Supply text or regex, not both."
        );
        assert!(call("browser_find", json!({ "regex": "S.e" })).is_ok());
    }

    #[test]
    fn emulate_media_tells_absent_null_and_value_apart() {
        let scheme = |args: Value| match call("browser_emulate_media", args) {
            Ok(ToolCall::EmulateMedia(media)) => media.color_scheme,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(scheme(json!({})), None);
        assert_eq!(scheme(json!({ "colorScheme": null })), Some(None));
        assert_eq!(
            scheme(json!({ "colorScheme": "dark" })),
            Some(Some(ColorScheme::Dark))
        );
    }

    #[test]
    fn plain_optional_fields_reject_null() {
        assert!(call("browser_click", json!({ "target": null })).is_err());
        assert!(call("browser_wait_for", json!({ "time": null, "text": "Done" })).is_err());
    }

    #[test]
    fn string_limits_count_utf16_units() {
        // One character outside the BMP is two UTF-16 units, so 127 of them fit in 255 and 128 do not.
        let astral = "\u{1D11E}";
        assert!(
            call(
                "browser_take_screenshot",
                json!({ "filename": astral.repeat(127) })
            )
            .is_ok()
        );
        assert_eq!(
            error(
                "browser_take_screenshot",
                json!({ "filename": astral.repeat(128) })
            ),
            "Invalid arguments for browser_take_screenshot: filename: Too big: expected string to have <=255 characters"
        );
    }

    #[test]
    fn defaults_fill_console_level_and_static() {
        let Ok(ToolCall::ConsoleMessages(console)) = call("browser_console_messages", json!({}))
        else {
            panic!("console messages should decode");
        };
        assert_eq!(console.level, ConsoleLevel::Info);
        let Ok(ToolCall::NetworkRequests(requests)) = call("browser_network_requests", json!({}))
        else {
            panic!("network requests should decode");
        };
        assert!(!requests.static_resources);
        let Ok(ToolCall::NetworkRequests(requests)) =
            call("browser_network_requests", json!({ "static": true }))
        else {
            panic!("the static key should decode");
        };
        assert!(requests.static_resources);
    }

    #[test]
    fn unsupported_names_are_cut_to_100_characters() {
        let long = "x".repeat(150);
        assert_eq!(
            error(&long, json!({})),
            format!("Unsupported native browser tool: {}.", "x".repeat(100))
        );
    }

    #[test]
    fn element_targets_follow_the_desktop_rules() {
        assert_eq!(
            error("browser_click", json!({ "target": "#a", "ref": "n1" })),
            "Use one target or matching ref, not conflicting targets."
        );
        assert_eq!(
            error("browser_hover", json!({})),
            "An exact snapshot ref or unique CSS target is required."
        );
        assert!(call("browser_click", json!({ "target": "#a", "ref": "#a" })).is_ok());
        assert_eq!(
            error(
                "browser_drag",
                json!({ "startTarget": "", "endTarget": "#b" })
            ),
            "An exact snapshot ref or unique CSS target is required."
        );
    }

    #[test]
    fn wait_for_needs_a_condition_and_time_up_to_20() {
        assert_eq!(
            error("browser_wait_for", json!({})),
            "Supply time, text or textGone."
        );
        assert!(call("browser_wait_for", json!({ "time": 0 })).is_ok());
        assert_eq!(
            error("browser_wait_for", json!({ "time": 60 })),
            "Invalid arguments for browser_wait_for: time: Too big: expected number to be <=20"
        );
    }

    #[test]
    fn network_request_index_is_a_safe_integer_from_1() {
        assert_eq!(
            error("browser_network_request", json!({ "index": 0 })),
            "Invalid arguments for browser_network_request: index: Too small: expected number to be >=1"
        );
        assert!(
            call(
                "browser_network_request",
                json!({ "index": MAX_SAFE_INTEGER })
            )
            .is_ok()
        );
        assert!(
            call(
                "browser_network_request",
                json!({ "index": MAX_SAFE_INTEGER + 1 })
            )
            .is_err()
        );
    }

    #[test]
    fn resize_and_tab_index_ranges_are_inclusive() {
        assert!(call("browser_resize", json!({ "width": 240, "height": 180 })).is_ok());
        assert!(call("browser_resize", json!({ "width": 2560, "height": 1600 })).is_ok());
        assert!(call("browser_resize", json!({ "width": 239, "height": 180 })).is_err());
        assert!(call("browser_resize", json!({ "width": 800, "height": 1601 })).is_err());
        assert!(call("browser_tabs", json!({ "action": "select", "index": 99 })).is_ok());
        assert!(call("browser_tabs", json!({ "action": "select", "index": 100 })).is_err());
    }
}
