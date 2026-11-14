//! Typed CDP methods and events the browser tools use, from the calls in `packages/desktop/src/native-browser-automation.ts`.
//! Parameters are written with the wire's camelCase names and carry only the fields the tools send. Replies and events
//! read the fields the tools use, default the ones the browser may leave out, and ignore the rest.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, DeserializeOwned, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::browser::cdp::{Method, NoParams, Nothing, method};

/// A JSON scalar a page reported: text, a boolean or a number. Null, arrays and objects read as `Other`, and their
/// contents are not kept.
#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    Text(String),
    Bool(bool),
    Number(f64),
    Other,
}

impl<'de> Deserialize<'de> for Scalar {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ScalarVisitor)
    }
}

struct ScalarVisitor;

impl<'de> Visitor<'de> for ScalarVisitor {
    type Value = Scalar;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Scalar, E> {
        Ok(Scalar::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Scalar, E> {
        Ok(Scalar::Number(value as f64))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Scalar, E> {
        Ok(Scalar::Number(value as f64))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Scalar, E> {
        Ok(Scalar::Number(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Scalar, E> {
        Ok(Scalar::Text(value.to_owned()))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Scalar, E> {
        Ok(Scalar::Other)
    }

    fn visit_none<E: de::Error>(self) -> Result<Scalar, E> {
        Ok(Scalar::Other)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Scalar, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Scalar::Other)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Scalar, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Scalar::Other)
    }
}

/// A header block in the order the browser sent it. Each value is its text: strings as they are, numbers and
/// booleans as their text, and anything else (null, arrays, objects) as empty. A missing block is empty.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Headers(pub Vec<(String, String)>);

impl<'de> Deserialize<'de> for Headers {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(HeadersVisitor)
    }
}

struct HeadersVisitor;

impl<'de> Visitor<'de> for HeadersVisitor {
    type Value = Headers;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object of header values")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Headers, A::Error> {
        let mut headers = Vec::new();
        while let Some((name, value)) = map.next_entry::<String, Scalar>()? {
            let text = match value {
                Scalar::Text(text) => text,
                Scalar::Bool(flag) => flag.to_string(),
                Scalar::Number(number) => number.to_string(),
                Scalar::Other => String::new(),
            };
            headers.push((name, text));
        }
        Ok(Headers(headers))
    }
}

// Accessibility

/// Parameters for `Accessibility.getFullAXTree`: how many levels of the tree to walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetFullAxTreeParams {
    pub depth: u32,
}

/// The reply to `Accessibility.getFullAXTree`: the tree as flat nodes that name their children by id.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GetFullAxTreeReply {
    pub nodes: Vec<AxNode>,
}

/// One node of the accessibility tree.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AxNode {
    pub node_id: String,
    pub ignored: bool,
    pub role: Option<AxValue>,
    pub name: Option<AxValue>,
    pub value: Option<AxValue>,
    #[serde(rename = "backendDOMNodeId")]
    pub backend_dom_node_id: Option<i64>,
    pub child_ids: Vec<String>,
    pub properties: Vec<AxProperty>,
}

/// A typed value on an accessibility node, such as its role or name.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct AxValue {
    pub value: Option<Scalar>,
}

/// A named property of an accessibility node.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AxProperty {
    pub name: String,
    pub value: AxValue,
}

method!(
    /// Walks the accessibility tree.
    AccessibilityGetFullAxTree, "Accessibility.getFullAXTree", GetFullAxTreeParams, GetFullAxTreeReply
);

// DOM

/// Parameters for `DOM.resolveNode`: a page object for a backend node, kept in `object_group`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveNodeParams {
    pub backend_node_id: i64,
    pub object_group: String,
}

/// The reply to `DOM.resolveNode`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResolveNodeReply {
    pub object: RemoteObjectRef,
}

/// A reference to a page object, by its id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemoteObjectRef {
    pub object_id: Option<String>,
}

/// Parameters for the DOM calls that take one page object: `DOM.describeNode` and `DOM.getBoxModel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectIdParams {
    pub object_id: String,
}

/// The reply to `DOM.describeNode`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DescribeNodeReply {
    pub node: DescribedNode,
}

/// The node a page object stands for, as far as the tools need it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DescribedNode {
    pub backend_node_id: Option<i64>,
}

/// The reply to `DOM.getBoxModel`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GetBoxModelReply {
    pub model: BoxModel,
}

/// A box in CSS pixels. `content` is the content quad: four corners as x, y pairs.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BoxModel {
    pub content: Vec<f64>,
}

/// Parameters for `DOM.getNodeForLocation`: the point to look under, in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeForLocationParams {
    pub x: i32,
    pub y: i32,
    #[serde(rename = "includeUserAgentShadowDOM")]
    pub include_user_agent_shadow_dom: bool,
}

/// The reply to `DOM.getNodeForLocation`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GetNodeForLocationReply {
    pub backend_node_id: Option<i64>,
}

/// Parameters for `DOM.setFileInputFiles`: the files to put in a file input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetFileInputFilesParams {
    pub backend_node_id: i64,
    pub files: Vec<String>,
}

method!(
    /// Turns on the DOM domain.
    DomEnable, "DOM.enable", NoParams, Nothing
);
method!(
    /// Makes a page object for a backend node.
    DomResolveNode, "DOM.resolveNode", ResolveNodeParams, ResolveNodeReply
);
method!(
    /// Describes the node a page object stands for.
    DomDescribeNode, "DOM.describeNode", ObjectIdParams, DescribeNodeReply
);
method!(
    /// Measures a page object's box.
    DomGetBoxModel, "DOM.getBoxModel", ObjectIdParams, GetBoxModelReply
);
method!(
    /// Finds the node under a point.
    DomGetNodeForLocation, "DOM.getNodeForLocation", NodeForLocationParams, GetNodeForLocationReply
);
method!(
    /// Puts files into a file input.
    DomSetFileInputFiles, "DOM.setFileInputFiles", SetFileInputFilesParams, Nothing
);

// Runtime

/// Parameters for `Runtime.releaseObjectGroup`: frees the page objects held in a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseObjectGroupParams {
    pub object_group: String,
}

/// Parameters for `Runtime.evaluate`: an expression to run in the page. Unset options are left out of the call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateParams {
    pub expression: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub await_promise: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_by_value: Option<bool>,
    /// Milliseconds before the browser gives up on the expression.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_gesture: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_group: Option<String>,
}

/// Parameters for `Runtime.callFunctionOn`: a function run with `object_id` as `this`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallFunctionOnParams {
    pub object_id: String,
    pub function_declaration: String,
    pub arguments: Vec<CallArgument>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub await_promise: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_by_value: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_gesture: Option<bool>,
}

/// One argument to a page function: a value passed by value, or a page object by its id.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CallArgument {
    /// Passed by value, as `{"value": ...}`.
    Value { value: ArgValue },
    /// A page object, as `{"objectId": "..."}`.
    Object {
        #[serde(rename = "objectId")]
        object_id: String,
    },
}

/// A value passed to a page function. Written bare, as the JSON value itself.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ArgValue {
    Bool(bool),
    Text(String),
    Texts(Vec<String>),
    Number(f64),
}

/// The reply to `Runtime.evaluate` and `Runtime.callFunctionOn`: the result, and the exception if one was thrown.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateReply<T> {
    #[serde(default = "Default::default")]
    pub result: RemoteObject<T>,
    pub exception_details: Option<ExceptionDetails>,
}

/// A value or page object the browser returned. `value` is set only when the call asked for it by value.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteObject<T> {
    pub object_id: Option<String>,
    pub value: Option<T>,
    pub description: Option<String>,
}

impl<T> Default for RemoteObject<T> {
    fn default() -> Self {
        Self {
            object_id: None,
            value: None,
            description: None,
        }
    }
}

/// The exception a script threw.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExceptionDetails {
    pub text: String,
    pub exception: Option<ExceptionValue>,
}

/// The thrown value, as far as the tools show it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ExceptionValue {
    pub description: Option<String>,
}

/// The reply of the actionability probe a tool runs in the page: `ok`, or the reason the element cannot take the action.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Readiness {
    pub ok: Option<bool>,
    pub error: Option<String>,
}

/// `Runtime.evaluate`, whose reply carries a `T` when the call asks for the value by value.
pub struct Evaluate<T>(PhantomData<T>);

impl<T: DeserializeOwned> Method for Evaluate<T> {
    const NAME: &'static str = "Runtime.evaluate";
    type Params = EvaluateParams;
    type Reply = EvaluateReply<T>;
}

/// `Runtime.callFunctionOn`, whose reply has the same shape as `Runtime.evaluate`'s.
pub struct CallFunctionOn<T>(PhantomData<T>);

impl<T: DeserializeOwned> Method for CallFunctionOn<T> {
    const NAME: &'static str = "Runtime.callFunctionOn";
    type Params = CallFunctionOnParams;
    type Reply = EvaluateReply<T>;
}

method!(
    /// Turns on the runtime domain, which evaluation needs.
    RuntimeEnable, "Runtime.enable", NoParams, Nothing
);
method!(
    /// Turns the runtime domain off (a private interaction cuts the bot off from the page's script).
    RuntimeDisable, "Runtime.disable", NoParams, Nothing
);
method!(
    /// Turns network observation off.
    NetworkDisable, "Network.disable", NoParams, Nothing
);
method!(
    /// Clears the console's stored messages.
    RuntimeDiscardConsoleEntries, "Runtime.discardConsoleEntries", NoParams, Nothing
);
method!(
    /// Stops the script that is running in the page.
    RuntimeTerminateExecution, "Runtime.terminateExecution", NoParams, Nothing
);
method!(
    /// Frees the page objects held in a group.
    RuntimeReleaseObjectGroup, "Runtime.releaseObjectGroup", ReleaseObjectGroupParams, Nothing
);

// Network

/// Parameters for `Network.enable`: how much request and response body the browser may keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkEnableParams {
    pub max_total_buffer_size: u32,
    pub max_resource_buffer_size: u32,
    pub max_post_data_size: u32,
}

/// Parameters for `Network.getResponseBody`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetResponseBodyParams {
    pub request_id: String,
}

/// The reply to `Network.getResponseBody`: the body as text, or base64 when it is binary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GetResponseBodyReply {
    pub body: String,
    pub base64_encoded: bool,
}

method!(
    /// Turns on network observation, keeping up to the given sizes of body.
    NetworkEnable, "Network.enable", NetworkEnableParams, Nothing
);
method!(
    /// Reads a response body the browser kept.
    NetworkGetResponseBody, "Network.getResponseBody", GetResponseBodyParams, GetResponseBodyReply
);

// Page and emulation

/// The reply to `Page.getLayoutMetrics`, in CSS pixels.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GetLayoutMetricsReply {
    pub css_visual_viewport: VisualViewport,
    pub css_content_size: ContentSize,
}

/// The part of the page the window shows.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VisualViewport {
    pub page_x: f64,
    pub page_y: f64,
    pub client_width: f64,
    pub client_height: f64,
}

/// The size of the whole page.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ContentSize {
    pub width: f64,
    pub height: f64,
}

/// Parameters for `Page.captureScreenshot`. `quality` and `clip` are left out when unset.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureScreenshotParams {
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip: Option<Clip>,
    pub capture_beyond_viewport: bool,
}

/// A region of the page to capture, in CSS pixels.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

/// The reply to `Page.captureScreenshot`: the image, base64 encoded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CaptureScreenshotReply {
    pub data: String,
}

/// Parameters for `Page.setInterceptFileChooserDialog`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetInterceptFileChooserParams {
    pub enabled: bool,
}

/// One media feature to emulate, such as `prefers-color-scheme: dark`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaFeature {
    pub name: String,
    pub value: String,
}

/// Parameters for `Emulation.setEmulatedMedia`: the media type (empty for none) and its features.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetEmulatedMediaParams {
    pub media: String,
    pub features: Vec<MediaFeature>,
}

method!(
    /// Gives the page's layout in CSS pixels.
    PageGetLayoutMetrics, "Page.getLayoutMetrics", NoParams, GetLayoutMetricsReply
);
method!(
    /// Captures the page, or a region of it, as an image.
    PageCaptureScreenshot, "Page.captureScreenshot", CaptureScreenshotParams, CaptureScreenshotReply
);
method!(
    /// Turns on the page domain, whose events (a file chooser opening) the tools wait for.
    PageEnable, "Page.enable", NoParams, Nothing
);
method!(
    /// Makes file-chooser dialogs report to the tool instead of opening.
    PageSetInterceptFileChooserDialog, "Page.setInterceptFileChooserDialog", SetInterceptFileChooserParams, Nothing
);
method!(
    /// Sets the media type and features the page sees.
    EmulationSetEmulatedMedia, "Emulation.setEmulatedMedia", SetEmulatedMediaParams, Nothing
);

// Input

/// The step of a drag that `Input.dispatchDragEvent` sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DragEventType {
    DragEnter,
    DragOver,
    Drop,
}

/// Parameters for `Input.dispatchDragEvent`: one step of a drag over the page at `x`, `y`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchDragParams {
    #[serde(rename = "type")]
    pub kind: DragEventType,
    pub x: f64,
    pub y: f64,
    pub data: DragData,
}

/// What a drag carries: items by MIME type, and files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DragData {
    pub items: Vec<DragItem>,
    pub files: Vec<String>,
    pub drag_operations_mask: u32,
}

/// One item a drag carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DragItem {
    pub mime_type: String,
    pub data: String,
}

method!(
    /// Sends one step of a drag over the page.
    InputDispatchDragEvent, "Input.dispatchDragEvent", DispatchDragParams, Nothing
);

// Events: decoded from the browser's notices, never subscribed to here.

/// `Runtime.consoleAPICalled`: the page wrote to the console.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConsoleApiCalled {
    #[serde(rename = "type")]
    pub kind: String,
    pub args: Vec<ConsoleArg>,
}

/// One argument the page passed to the console.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ConsoleArg {
    pub description: Option<String>,
    pub value: Option<Scalar>,
}

/// `Runtime.exceptionThrown`: the page threw an error that nothing caught.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExceptionThrown {
    pub exception_details: ExceptionDetails,
}

/// `Network.requestWillBeSent`: the page is about to send a request.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RequestWillBeSent {
    pub request_id: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub request: RequestInfo,
}

/// The request a `Network.requestWillBeSent` event describes.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RequestInfo {
    pub url: String,
    pub method: String,
    pub headers: Headers,
}

/// `Network.responseReceived`: the browser has the head of a response.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResponseReceived {
    pub request_id: String,
    pub response: ResponseInfo,
}

/// The response a `Network.responseReceived` event describes.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResponseInfo {
    pub status: Option<f64>,
    pub headers: Headers,
}

/// `Page.fileChooserOpened`: a file chooser opened. `backend_node_id` is its input, when the browser knows it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChooserOpened {
    pub backend_node_id: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire<T: Serialize>(value: &T) -> String {
        serde_json::to_string(value).unwrap()
    }

    fn decode<T: DeserializeOwned>(json: &str) -> T {
        serde_json::from_str(json).unwrap()
    }

    fn text(value: &str) -> Scalar {
        Scalar::Text(value.into())
    }

    #[test]
    fn method_names_are_the_cdp_names() {
        assert_eq!(
            AccessibilityGetFullAxTree::NAME,
            "Accessibility.getFullAXTree"
        );
        assert_eq!(DomEnable::NAME, "DOM.enable");
        assert_eq!(DomResolveNode::NAME, "DOM.resolveNode");
        assert_eq!(DomDescribeNode::NAME, "DOM.describeNode");
        assert_eq!(DomGetBoxModel::NAME, "DOM.getBoxModel");
        assert_eq!(DomGetNodeForLocation::NAME, "DOM.getNodeForLocation");
        assert_eq!(DomSetFileInputFiles::NAME, "DOM.setFileInputFiles");
        assert_eq!(RuntimeEnable::NAME, "Runtime.enable");
        assert_eq!(
            RuntimeDiscardConsoleEntries::NAME,
            "Runtime.discardConsoleEntries"
        );
        assert_eq!(
            RuntimeTerminateExecution::NAME,
            "Runtime.terminateExecution"
        );
        assert_eq!(
            RuntimeReleaseObjectGroup::NAME,
            "Runtime.releaseObjectGroup"
        );
        assert_eq!(<Evaluate<bool> as Method>::NAME, "Runtime.evaluate");
        assert_eq!(
            <CallFunctionOn<bool> as Method>::NAME,
            "Runtime.callFunctionOn"
        );
        assert_eq!(NetworkEnable::NAME, "Network.enable");
        assert_eq!(NetworkGetResponseBody::NAME, "Network.getResponseBody");
        assert_eq!(PageGetLayoutMetrics::NAME, "Page.getLayoutMetrics");
        assert_eq!(PageCaptureScreenshot::NAME, "Page.captureScreenshot");
        assert_eq!(
            PageSetInterceptFileChooserDialog::NAME,
            "Page.setInterceptFileChooserDialog"
        );
        assert_eq!(
            EmulationSetEmulatedMedia::NAME,
            "Emulation.setEmulatedMedia"
        );
        assert_eq!(InputDispatchDragEvent::NAME, "Input.dispatchDragEvent");
    }

    #[test]
    fn accessibility_and_dom_params_are_written_as_cdp_reads_them() {
        assert_eq!(wire(&GetFullAxTreeParams { depth: 14 }), r#"{"depth":14}"#);
        assert_eq!(
            wire(&ResolveNodeParams {
                backend_node_id: 7,
                object_group: "tjommie-tools".into()
            }),
            r#"{"backendNodeId":7,"objectGroup":"tjommie-tools"}"#
        );
        assert_eq!(
            wire(&ObjectIdParams {
                object_id: "obj-1".into()
            }),
            r#"{"objectId":"obj-1"}"#
        );
        assert_eq!(
            wire(&NodeForLocationParams {
                x: 10,
                y: -4,
                include_user_agent_shadow_dom: true
            }),
            r#"{"x":10,"y":-4,"includeUserAgentShadowDOM":true}"#
        );
        assert_eq!(
            wire(&SetFileInputFilesParams {
                backend_node_id: 7,
                files: vec!["C:/a.txt".into()]
            }),
            r#"{"backendNodeId":7,"files":["C:/a.txt"]}"#
        );
    }

    #[test]
    fn runtime_and_network_params_are_written_as_cdp_reads_them() {
        assert_eq!(wire(&NoParams {}), "{}");
        assert_eq!(
            wire(&ReleaseObjectGroupParams {
                object_group: "tjommie-tools".into()
            }),
            r#"{"objectGroup":"tjommie-tools"}"#
        );
        assert_eq!(
            wire(&NetworkEnableParams {
                max_total_buffer_size: 1,
                max_resource_buffer_size: 2,
                max_post_data_size: 0,
            }),
            r#"{"maxTotalBufferSize":1,"maxResourceBufferSize":2,"maxPostDataSize":0}"#
        );
        assert_eq!(
            wire(&GetResponseBodyParams {
                request_id: "r1".into()
            }),
            r#"{"requestId":"r1"}"#
        );
        assert_eq!(
            wire(&SetInterceptFileChooserParams { enabled: false }),
            r#"{"enabled":false}"#
        );
    }

    #[test]
    fn evaluate_params_leave_out_what_is_not_set() {
        let bare = EvaluateParams {
            expression: "1+1".into(),
            await_promise: None,
            return_by_value: None,
            timeout: None,
            user_gesture: None,
            object_group: None,
        };
        assert_eq!(wire(&bare), r#"{"expression":"1+1"}"#);
        let full = EvaluateParams {
            expression: "document.title".into(),
            await_promise: Some(true),
            return_by_value: Some(false),
            timeout: Some(8000),
            user_gesture: Some(true),
            object_group: Some("tjommie-tools".into()),
        };
        assert_eq!(
            wire(&full),
            r#"{"expression":"document.title","awaitPromise":true,"returnByValue":false,"timeout":8000,"userGesture":true,"objectGroup":"tjommie-tools"}"#
        );
    }

    #[test]
    fn call_function_on_arguments_have_their_two_wire_shapes() {
        let params = CallFunctionOnParams {
            object_id: "obj-1".into(),
            function_declaration: "function(hit) { return this === hit; }".into(),
            arguments: vec![
                CallArgument::Object {
                    object_id: "obj-2".into(),
                },
                CallArgument::Value {
                    value: ArgValue::Bool(true),
                },
                CallArgument::Value {
                    value: ArgValue::Text("a".into()),
                },
                CallArgument::Value {
                    value: ArgValue::Texts(vec!["x".into(), "y".into()]),
                },
                CallArgument::Value {
                    value: ArgValue::Number(2.5),
                },
            ],
            await_promise: None,
            return_by_value: Some(true),
            user_gesture: None,
        };
        assert_eq!(
            wire(&params),
            r#"{"objectId":"obj-1","functionDeclaration":"function(hit) { return this === hit; }","arguments":[{"objectId":"obj-2"},{"value":true},{"value":"a"},{"value":["x","y"]},{"value":2.5}],"returnByValue":true}"#
        );
        let no_arguments = CallFunctionOnParams {
            object_id: "obj-1".into(),
            function_declaration: "function() { return 1; }".into(),
            arguments: Vec::new(),
            await_promise: Some(true),
            return_by_value: None,
            user_gesture: Some(false),
        };
        assert_eq!(
            wire(&no_arguments),
            r#"{"objectId":"obj-1","functionDeclaration":"function() { return 1; }","arguments":[],"awaitPromise":true,"userGesture":false}"#
        );
    }

    #[test]
    fn screenshot_params_leave_out_quality_and_clip_when_unset() {
        let bare = CaptureScreenshotParams {
            format: "png".into(),
            quality: None,
            clip: None,
            capture_beyond_viewport: false,
        };
        assert_eq!(
            wire(&bare),
            r#"{"format":"png","captureBeyondViewport":false}"#
        );
        let clipped = CaptureScreenshotParams {
            format: "jpeg".into(),
            quality: Some(80),
            clip: Some(Clip {
                x: 0.0,
                y: 10.0,
                width: 100.0,
                height: 50.0,
                scale: 1.0,
            }),
            capture_beyond_viewport: true,
        };
        assert_eq!(
            wire(&clipped),
            r#"{"format":"jpeg","quality":80,"clip":{"x":0.0,"y":10.0,"width":100.0,"height":50.0,"scale":1.0},"captureBeyondViewport":true}"#
        );
    }

    #[test]
    fn page_emulation_and_input_params_are_written_as_cdp_reads_them() {
        assert_eq!(
            wire(&SetEmulatedMediaParams {
                media: "print".into(),
                features: vec![MediaFeature {
                    name: "prefers-color-scheme".into(),
                    value: "dark".into()
                }],
            }),
            r#"{"media":"print","features":[{"name":"prefers-color-scheme","value":"dark"}]}"#
        );
        assert_eq!(
            wire(&DispatchDragParams {
                kind: DragEventType::DragEnter,
                x: 3.0,
                y: 4.5,
                data: DragData {
                    items: vec![DragItem {
                        mime_type: "text/plain".into(),
                        data: "hi".into()
                    }],
                    files: vec!["C:/a.txt".into()],
                    drag_operations_mask: 1,
                },
            }),
            r#"{"type":"dragEnter","x":3.0,"y":4.5,"data":{"items":[{"mimeType":"text/plain","data":"hi"}],"files":["C:/a.txt"],"dragOperationsMask":1}}"#
        );
        assert_eq!(wire(&DragEventType::DragOver), r#""dragOver""#);
        assert_eq!(wire(&DragEventType::Drop), r#""drop""#);
    }

    #[test]
    fn ax_tree_reply_reads_nodes_and_defaults_the_rest() {
        let tree: GetFullAxTreeReply = decode(
            r#"{"nodes":[
                {"nodeId":"1","ignored":false,"role":{"type":"role","value":"button"},
                 "name":{"type":"computedString","value":"Save"},"backendDOMNodeId":11,
                 "childIds":["2"],"frameId":"F1","properties":[
                   {"name":"focusable","value":{"type":"booleanOrUndefined","value":true}},
                   {"name":"level","value":{"type":"integer","value":2}}]},
                {"nodeId":"2","ignored":true,"value":{"type":"string"}}],"extra":1}"#,
        );
        assert_eq!(tree.nodes.len(), 2);
        let button = &tree.nodes[0];
        assert_eq!(button.node_id, "1");
        assert!(!button.ignored);
        assert_eq!(
            button.role,
            Some(AxValue {
                value: Some(text("button"))
            })
        );
        assert_eq!(
            button.name,
            Some(AxValue {
                value: Some(text("Save"))
            })
        );
        assert_eq!(button.backend_dom_node_id, Some(11));
        assert_eq!(button.child_ids, vec!["2".to_string()]);
        assert_eq!(
            button.properties[1],
            AxProperty {
                name: "level".into(),
                value: AxValue {
                    value: Some(Scalar::Number(2.0))
                }
            }
        );
        let hidden = &tree.nodes[1];
        assert!(hidden.ignored);
        assert_eq!(hidden.value, Some(AxValue { value: None }));
        assert_eq!(hidden.backend_dom_node_id, None);
        assert!(hidden.properties.is_empty());
        assert_eq!(
            decode::<GetFullAxTreeReply>(r#"{"nodes":[{"nodeId":"9"}]}"#).nodes[0],
            AxNode {
                node_id: "9".into(),
                ..AxNode::default()
            }
        );
        assert!(decode::<GetFullAxTreeReply>("{}").nodes.is_empty());
    }

    #[test]
    fn dom_replies_read_the_ids_the_tools_need() {
        let resolved: ResolveNodeReply = decode(
            r#"{"object":{"type":"object","subtype":"node","className":"HTMLButtonElement","objectId":"obj-3"},"extra":true}"#,
        );
        assert_eq!(resolved.object.object_id.as_deref(), Some("obj-3"));
        assert_eq!(decode::<ResolveNodeReply>("{}").object.object_id, None);

        let described: DescribeNodeReply =
            decode(r#"{"node":{"nodeId":4,"backendNodeId":11,"nodeName":"BUTTON"}}"#);
        assert_eq!(described.node.backend_node_id, Some(11));
        assert_eq!(decode::<DescribeNodeReply>("{}").node.backend_node_id, None);

        let boxed: GetBoxModelReply = decode(
            r#"{"model":{"content":[1,2,11,2,11,9,1,9],"padding":[1,2,11,2,11,9,1,9],"width":10,"height":7}}"#,
        );
        assert_eq!(
            boxed.model.content,
            vec![1.0, 2.0, 11.0, 2.0, 11.0, 9.0, 1.0, 9.0]
        );
        assert!(decode::<GetBoxModelReply>("{}").model.content.is_empty());

        let located: GetNodeForLocationReply =
            decode(r#"{"backendNodeId":12,"frameId":"F1","nodeId":0}"#);
        assert_eq!(located.backend_node_id, Some(12));
        assert_eq!(
            decode::<GetNodeForLocationReply>("{}").backend_node_id,
            None
        );
    }

    #[test]
    fn evaluate_reply_reads_the_value_and_the_exception() {
        let ok: EvaluateReply<Readiness> = decode(
            r#"{"result":{"type":"object","value":{"ok":true},"description":"Object"},"extra":1}"#,
        );
        assert_eq!(
            ok.result.value,
            Some(Readiness {
                ok: Some(true),
                error: None
            })
        );
        assert_eq!(ok.result.description.as_deref(), Some("Object"));
        assert!(ok.exception_details.is_none());

        let refused: EvaluateReply<Readiness> =
            decode(r#"{"result":{"type":"object","value":{"error":"The element is disabled."}}}"#);
        assert_eq!(
            refused
                .result
                .value
                .as_ref()
                .and_then(|ready| ready.error.as_deref()),
            Some("The element is disabled.")
        );

        let thrown: EvaluateReply<bool> = decode(
            r#"{"result":{"type":"undefined"},"exceptionDetails":{"exceptionId":1,"text":"Uncaught","lineNumber":0,"columnNumber":4,"exception":{"type":"object","description":"Error: boom"}}}"#,
        );
        assert_eq!(thrown.result, RemoteObject::default());
        assert_eq!(
            thrown.exception_details,
            Some(ExceptionDetails {
                text: "Uncaught".into(),
                exception: Some(ExceptionValue {
                    description: Some("Error: boom".into())
                }),
            })
        );

        let bare: EvaluateReply<bool> = decode("{}");
        assert_eq!(bare.result, RemoteObject::default());
        assert!(bare.exception_details.is_none());
    }

    #[test]
    fn network_page_and_screenshot_replies_read_their_fields() {
        let body: GetResponseBodyReply =
            decode(r#"{"body":"aGk=","base64Encoded":true,"extra":"x"}"#);
        assert_eq!(
            body,
            GetResponseBodyReply {
                body: "aGk=".into(),
                base64_encoded: true
            }
        );
        let plain: GetResponseBodyReply = decode(r#"{"body":"hi"}"#);
        assert_eq!(
            plain,
            GetResponseBodyReply {
                body: "hi".into(),
                base64_encoded: false
            }
        );

        let metrics: GetLayoutMetricsReply = decode(
            r#"{"layoutViewport":{"clientWidth":800},"cssVisualViewport":{"pageX":0,"pageY":120.5,"clientWidth":800,"clientHeight":600,"scale":1},"cssContentSize":{"width":800,"height":4000}}"#,
        );
        assert_eq!(metrics.css_visual_viewport.page_y, 120.5);
        assert_eq!(metrics.css_visual_viewport.client_height, 600.0);
        assert_eq!(metrics.css_content_size.height, 4000.0);
        let empty: GetLayoutMetricsReply = decode("{}");
        assert_eq!(empty.css_visual_viewport.client_height, 0.0);
        assert_eq!(empty.css_content_size.width, 0.0);

        let shot: CaptureScreenshotReply = decode(r#"{"data":"iVBORw0KGgo=","extra":1}"#);
        assert_eq!(shot.data, "iVBORw0KGgo=");
        assert_eq!(decode::<CaptureScreenshotReply>("{}").data, "");
    }

    #[test]
    fn replies_with_nothing_in_them_decode_from_an_object() {
        let _: Nothing = decode(r#"{"extra":1}"#);
        let _: Nothing = decode("{}");
    }

    #[test]
    fn console_and_exception_events_decode() {
        let console: ConsoleApiCalled = decode(
            r#"{"type":"warning","args":[{"type":"string","value":"careful"},{"type":"object","description":"Error: x","objectId":"obj-9"},{"type":"number","value":3},{"type":"object","value":{"a":1}}],"executionContextId":2,"timestamp":1.5,"stackTrace":{"callFrames":[]}}"#,
        );
        assert_eq!(console.kind, "warning");
        assert_eq!(console.args.len(), 4);
        assert_eq!(console.args[0].value, Some(text("careful")));
        assert_eq!(console.args[1].description.as_deref(), Some("Error: x"));
        assert_eq!(console.args[1].value, None);
        assert_eq!(console.args[2].value, Some(Scalar::Number(3.0)));
        assert_eq!(console.args[3].value, Some(Scalar::Other));
        let bare: ConsoleApiCalled = decode("{}");
        assert_eq!(bare.kind, "");
        assert!(bare.args.is_empty());

        let thrown: ExceptionThrown = decode(
            r#"{"timestamp":1,"exceptionDetails":{"exceptionId":4,"text":"Uncaught TypeError","lineNumber":2,"columnNumber":0,"exception":{"type":"object","description":"TypeError: x"}}}"#,
        );
        assert_eq!(thrown.exception_details.text, "Uncaught TypeError");
        assert_eq!(
            thrown.exception_details.exception,
            Some(ExceptionValue {
                description: Some("TypeError: x".into())
            })
        );
        assert_eq!(decode::<ExceptionThrown>("{}").exception_details.text, "");
    }

    #[test]
    fn request_and_response_events_decode_with_headers() {
        let request: RequestWillBeSent = decode(
            r#"{"requestId":"r1","loaderId":"l","documentURL":"https://a.test/","request":{"url":"https://a.test/api","method":"POST","headers":{"Content-Type":"application/json","X-Count":2},"postData":"{}"},"timestamp":3,"type":"Fetch","initiator":{"type":"script"}}"#,
        );
        assert_eq!(request.request_id, "r1");
        assert_eq!(request.kind.as_deref(), Some("Fetch"));
        assert_eq!(request.request.url, "https://a.test/api");
        assert_eq!(request.request.method, "POST");
        assert_eq!(
            request.request.headers,
            Headers(vec![
                ("Content-Type".into(), "application/json".into()),
                ("X-Count".into(), "2".into()),
            ])
        );
        let bare: RequestWillBeSent = decode(r#"{"requestId":"r2"}"#);
        assert_eq!(bare.kind, None);
        assert_eq!(bare.request, RequestInfo::default());

        let response: ResponseReceived = decode(
            r#"{"requestId":"r1","loaderId":"l","type":"Document","response":{"url":"https://a.test/api","status":201,"statusText":"Created","headers":{"Set-Cookie":"s=1"},"mimeType":"text/html"},"timestamp":4}"#,
        );
        assert_eq!(response.request_id, "r1");
        assert_eq!(response.response.status, Some(201.0));
        assert_eq!(
            response.response.headers,
            Headers(vec![("Set-Cookie".into(), "s=1".into())])
        );
        let bare: ResponseReceived = decode(r#"{"requestId":"r2"}"#);
        assert_eq!(bare.response.status, None);
        assert_eq!(bare.response.headers, Headers::default());
    }

    #[test]
    fn file_chooser_event_decodes_the_node() {
        let opened: FileChooserOpened =
            decode(r#"{"frameId":"F1","backendNodeId":21,"mode":"selectSingle"}"#);
        assert_eq!(opened.backend_node_id, Some(21));
        let no_node: FileChooserOpened = decode(r#"{"frameId":"F1","mode":"selectMultiple"}"#);
        assert_eq!(no_node.backend_node_id, None);
    }

    #[test]
    fn scalar_keeps_text_bool_and_number_and_nothing_else() {
        assert_eq!(decode::<Scalar>(r#""Save""#), text("Save"));
        assert_eq!(decode::<Scalar>("true"), Scalar::Bool(true));
        assert_eq!(decode::<Scalar>("-2.5"), Scalar::Number(-2.5));
        assert_eq!(decode::<Scalar>("42"), Scalar::Number(42.0));
        assert_eq!(decode::<Scalar>("null"), Scalar::Other);
        assert_eq!(decode::<Scalar>(r#"["a"]"#), Scalar::Other);
        assert_eq!(decode::<Scalar>(r#"{"k":"v"}"#), Scalar::Other);
        assert_eq!(decode::<Option<Scalar>>("null"), None);
    }

    #[test]
    fn headers_keep_their_order_and_the_text_of_each_value() {
        let headers: Headers = decode(
            r#"{"set-cookie":"a=1","content-length":42,"x-cached":true,"x-null":null,"x-list":["a"],"x-object":{"k":1}}"#,
        );
        assert_eq!(
            headers,
            Headers(vec![
                ("set-cookie".into(), "a=1".into()),
                ("content-length".into(), "42".into()),
                ("x-cached".into(), "true".into()),
                ("x-null".into(), String::new()),
                ("x-list".into(), String::new()),
                ("x-object".into(), String::new()),
            ])
        );
        assert_eq!(decode::<Headers>("{}"), Headers::default());
    }
}
