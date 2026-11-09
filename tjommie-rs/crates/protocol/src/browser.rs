//! `browser.ts`

use serde::{Deserialize, Serialize};

pub const BROWSER_START_URL: &str = "https://duckduckgo.com/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserControl {
    Human,
    Bot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserTab {
    pub id: String,
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDialog {
    pub page_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
    pub default_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserState {
    pub bot_id: String,
    pub generation: String,
    pub control: BrowserControl,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_control: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<bool>,
    pub active_page_id: Option<String>,
    pub tabs: Vec<BrowserTab>,
    pub dialog: Option<BrowserDialog>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseAction {
    Move,
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyAction {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// Coordinates are CSS pixels in the page's viewport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum BrowserInput {
    Mouse {
        action: MouseAction,
        x: f64,
        y: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<MouseButton>,
    },
    Wheel {
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
    },
    Key {
        action: KeyAction,
        key: String,
    },
    Text {
        text: String,
    },
}

/// The literal `"image/jpeg"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FrameMime {
    #[serde(rename = "image/jpeg")]
    Jpeg,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserFrame {
    /// Standard base64.
    pub data: String,
    pub mime: FrameMime,
    pub width: u32,
    pub height: u32,
    pub sequence: u64,
    /// Encoded pixels can be downsampled. Input always uses the separately reported CSS viewport.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport_height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_epoch: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotTarget {
    pub bot_id: String,
}

/// A bot's browser session and, for page actions, the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageTarget {
    pub bot_id: String,
    pub generation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_epoch: Option<u64>,
    pub page_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameCursor {
    pub stream_id: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserFrameParams {
    pub bot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<FrameCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserFrameResult {
    pub state: BrowserState,
    pub frame: Option<BrowserFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unchanged: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserControlParams {
    pub bot_id: String,
    pub control: BrowserControl,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_only: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserNavigate {
    #[serde(flatten)]
    pub page: PageTarget,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryAction {
    Back,
    Forward,
    Reload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserHistory {
    #[serde(flatten)]
    pub page: PageTarget,
    pub action: HistoryAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabAction {
    New,
    Select,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserTabParams {
    pub bot_id: String,
    pub generation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_epoch: Option<u64>,
    pub action: TabAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserResize {
    #[serde(flatten)]
    pub page: PageTarget,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserInputParams {
    #[serde(flatten)]
    pub page: PageTarget,
    pub input: BrowserInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserDialogParams {
    #[serde(flatten)]
    pub page: PageTarget,
    pub accept: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_text: Option<String>,
}
