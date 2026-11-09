//! `native-browser.ts`: commands for the desktop's native browser.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Wire;
use crate::browser::{BrowserControl, BrowserInput, HistoryAction, TabAction};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeBrowserBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeBrowserMount {
    pub visible: bool,
    pub bounds: NativeBrowserBounds,
    /// A temporary overlay, not leaving the browser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occluded: Option<bool>,
}

impl Wire for NativeBrowserMount {
    const NAME: &'static str = "NativeBrowserMount";
}

/// What renderer IPC accepts. Tool commands belong to the private desktop-host bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum NativeBrowserUiCommand {
    Open,
    State,
    Control {
        control: BrowserControl,
    },
    Navigate {
        generation: String,
        page_id: String,
        url: String,
    },
    History {
        generation: String,
        page_id: String,
        action: HistoryAction,
    },
    Tab {
        generation: String,
        action: TabAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        page_id: Option<String>,
    },
    Dialog {
        generation: String,
        page_id: String,
        accept: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_text: Option<String>,
    },
}

impl Wire for NativeBrowserUiCommand {
    const NAME: &'static str = "NativeBrowserUiCommand";
}

/// A browser tool's arguments, passed through to the desktop host unread. At most 1,000,000 characters as JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ToolArgs(Map<String, Value>);

impl ToolArgs {
    pub fn new(args: Map<String, Value>) -> Result<Self, crate::Invalid> {
        // JSON.stringify(args).length counts UTF-16 units; serde_json writes the same text.
        let json = serde_json::to_string(&args).expect("a JSON map serializes");
        if json.encode_utf16().count() <= 1_000_000 {
            Ok(Self(args))
        } else {
            Err(crate::Invalid(
                "Browser tool arguments exceed the size limit".into(),
            ))
        }
    }

    pub fn as_map(&self) -> &Map<String, Value> {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ToolArgs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Map::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeBrowserToolCommand {
    /// `browser_` and a snake_case tool name.
    pub name: String,
    pub args: ToolArgs,
}

/// What a remote controller may do. Only the authenticated daemon adds the controller id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum NativeBrowserRemoteAction {
    State,
    Frame {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thumbnail: Option<bool>,
    },
    Disconnect,
    Control {
        control: BrowserControl,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        generation: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        control_epoch: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        release_only: Option<bool>,
    },
    Navigate {
        control_epoch: u64,
        generation: String,
        page_id: String,
        url: String,
    },
    History {
        control_epoch: u64,
        generation: String,
        page_id: String,
        action: HistoryAction,
    },
    Tab {
        generation: String,
        control_epoch: u64,
        action: TabAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        page_id: Option<String>,
    },
    Dialog {
        control_epoch: u64,
        generation: String,
        page_id: String,
        accept: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt_text: Option<String>,
    },
    Resize {
        control_epoch: u64,
        generation: String,
        page_id: String,
        width: u32,
        height: u32,
    },
    Input {
        control_epoch: u64,
        generation: String,
        page_id: String,
        viewport_revision: u64,
        input: BrowserInput,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeBrowserRemoteCommand {
    pub controller_id: String,
    pub action: NativeBrowserRemoteAction,
}

/// Commands only the desktop-host bridge accepts, never renderer IPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NativeBrowserHostCommand {
    Tool(NativeBrowserToolCommand),
    Remote(NativeBrowserRemoteCommand),
    Close,
    DeleteProfile,
    CancelTools,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NativeBrowserCommand {
    Ui(NativeBrowserUiCommand),
    Host(NativeBrowserHostCommand),
}

impl Wire for NativeBrowserCommand {
    const NAME: &'static str = "NativeBrowserCommand";
}
