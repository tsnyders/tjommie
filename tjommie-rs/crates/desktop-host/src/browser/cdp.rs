//! Typed Chrome DevTools Protocol calls. WebView2 takes and returns JSON text; each CDP method used here is a type
//! with typed parameters and a typed reply, so untyped JSON exists only inside this boundary (the decode of a reply).

use serde::Serialize;
use serde::de::DeserializeOwned;
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;

use super::com::{self, ComError};

/// One CDP method.
pub trait Method {
    const NAME: &'static str;
    type Params: Serialize;
    type Reply: DeserializeOwned;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdpError {
    /// The browser refused or failed the command.
    Call(ComError),
    /// The parameters could not be written, or the reply did not have the expected shape.
    Shape(String),
}

impl std::fmt::Display for CdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Call(error) => write!(f, "{error}"),
            Self::Shape(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CdpError {}

impl From<ComError> for CdpError {
    fn from(error: ComError) -> Self {
        Self::Call(error)
    }
}

/// A reply with nothing in it that matters (`{}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
pub struct Nothing {}

/// Parameters with no fields (`{}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NoParams {}

/// Counts the bytes written to it and keeps none.
struct Count(usize);

impl std::io::Write for Count {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The command's parameters as JSON, in memory that is wiped when it drops. A command may carry a secret (a login being
/// filled), so the buffer is sized first and written once: a buffer that grew would leave copies behind that nothing wipes.
pub(crate) fn encode<T: Serialize>(
    params: &T,
) -> Result<zeroize::Zeroizing<Vec<u8>>, serde_json::Error> {
    let mut size = Count(0);
    serde_json::to_writer(&mut size, params)?;
    let mut json = zeroize::Zeroizing::new(Vec::with_capacity(size.0));
    serde_json::to_writer(&mut *json, params)?;
    Ok(json)
}

/// Runs one CDP method on a page.
pub async fn call<M: Method>(
    webview: &ICoreWebView2,
    params: &M::Params,
) -> Result<M::Reply, CdpError> {
    let encoded =
        encode(params).map_err(|error| CdpError::Shape(format!("{}: {error}", M::NAME)))?;
    let encoded = std::str::from_utf8(&encoded)
        .map_err(|error| CdpError::Shape(format!("{}: {error}", M::NAME)))?;
    let reply = com::call_devtools(webview, M::NAME, encoded).await?;
    // An empty reply stands for `{}`.
    let reply = if reply.trim().is_empty() {
        "{}"
    } else {
        reply.as_str()
    };
    serde_json::from_str(reply).map_err(|error| {
        CdpError::Shape(format!("{} returned an unexpected reply: {error}", M::NAME))
    })
}

macro_rules! method {
    ($(#[$doc:meta])* $name:ident, $wire:literal, $params:ty, $reply:ty) => {
        $(#[$doc])*
        pub struct $name;
        impl Method for $name {
            const NAME: &'static str = $wire;
            type Params = $params;
            type Reply = $reply;
        }
    };
}
pub(crate) use method;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDeviceMetricsParams {
    pub width: u32,
    pub height: u32,
    /// 0 keeps the page's own device scale factor.
    pub device_scale_factor: f64,
    pub mobile: bool,
}
method!(
    /// Fixes the page's viewport size, whatever the window's.
    EmulationSetDeviceMetricsOverride, "Emulation.setDeviceMetricsOverride", SetDeviceMetricsParams, Nothing
);
method!(
    /// Gives the page its window's own viewport again.
    EmulationClearDeviceMetricsOverride, "Emulation.clearDeviceMetricsOverride", NoParams, Nothing
);

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertTextParams {
    pub text: String,
}
method!(
    InputInsertText,
    "Input.insertText",
    InsertTextParams,
    Nothing
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyEventType {
    KeyDown,
    KeyUp,
    RawKeyDown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchKeyParams {
    #[serde(rename = "type")]
    pub kind: KeyEventType,
    /// 1 alt, 2 control, 4 meta, 8 shift.
    pub modifiers: u32,
    pub key: String,
    pub code: String,
    pub windows_virtual_key_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}
method!(
    InputDispatchKeyEvent,
    "Input.dispatchKeyEvent",
    DispatchKeyParams,
    Nothing
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseEventType {
    MouseMoved,
    MousePressed,
    MouseReleased,
    MouseWheel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PointerButton {
    None,
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchMouseParams {
    #[serde(rename = "type")]
    pub kind: MouseEventType,
    pub x: f64,
    pub y: f64,
    pub modifiers: u32,
    pub button: PointerButton,
    /// The buttons held after this event: 1 left, 2 right, 4 middle.
    pub buttons: u32,
    pub click_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_y: Option<f64>,
}
method!(
    InputDispatchMouseEvent,
    "Input.dispatchMouseEvent",
    DispatchMouseParams,
    Nothing
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_encoded_command_is_one_exact_allocation() {
        // A long value, as a passphrase may be: a buffer that grew by doubling would leave its first copies behind, and
        // only the last one would be wiped.
        let params = InsertTextParams {
            text: "p".repeat(5000),
        };
        let encoded = encode(&params).unwrap();
        assert_eq!(
            encoded.as_slice(),
            serde_json::to_vec(&params).unwrap().as_slice()
        );
        assert_eq!(
            encoded.capacity(),
            encoded.len(),
            "growth leaves unwiped copies"
        );
    }

    #[test]
    fn parameters_are_written_the_way_cdp_reads_them() {
        let key = DispatchKeyParams {
            kind: KeyEventType::KeyDown,
            modifiers: 8,
            key: "A".into(),
            code: "KeyA".into(),
            windows_virtual_key_code: 65,
            text: Some("A".into()),
        };
        assert_eq!(
            serde_json::to_string(&key).unwrap(),
            r#"{"type":"keyDown","modifiers":8,"key":"A","code":"KeyA","windowsVirtualKeyCode":65,"text":"A"}"#
        );
        let mouse = DispatchMouseParams {
            kind: MouseEventType::MouseWheel,
            x: 10.0,
            y: 20.5,
            modifiers: 0,
            button: PointerButton::None,
            buttons: 0,
            click_count: 0,
            delta_x: Some(0.0),
            delta_y: Some(-120.0),
        };
        assert_eq!(
            serde_json::to_string(&mouse).unwrap(),
            r#"{"type":"mouseWheel","x":10.0,"y":20.5,"modifiers":0,"button":"none","buttons":0,"clickCount":0,"deltaX":0.0,"deltaY":-120.0}"#
        );
        let metrics = SetDeviceMetricsParams {
            width: 800,
            height: 600,
            device_scale_factor: 0.0,
            mobile: false,
        };
        assert_eq!(
            serde_json::to_string(&metrics).unwrap(),
            r#"{"width":800,"height":600,"deviceScaleFactor":0.0,"mobile":false}"#
        );
    }
}
