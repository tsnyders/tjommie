//! The bounded accessibility snapshot with document-scoped refs (`snapshot` in `native-browser-automation.ts`), and the
//! search over it (`browser_find`). A password's value is never in a snapshot: the accessibility tree marks the node
//! protected and the line leaves its value out.

use std::collections::{BTreeSet, HashMap, HashSet};

use super::protocol::{AccessibilityGetFullAxTree, AxNode, AxValue, GetFullAxTreeParams, Scalar};
use super::reply::{TEXT_LIMIT, ToolError, bounded, simple_regex};
use super::run::{Run, js_round};

/// Nodes a snapshot lists at most.
const NODE_LIMIT: usize = 800;
/// Boxes are measured for the first this many listed nodes only.
const BOX_LIMIT: usize = 80;
/// The properties a line shows.
const SHOWN: [&str; 6] = [
    "checked", "disabled", "expanded", "selected", "readonly", "required",
];

fn json(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// A scalar the way JavaScript's `String()` writes it.
fn js_text(value: &Scalar) -> String {
    match value {
        Scalar::Text(text) => text.clone(),
        Scalar::Bool(flag) => flag.to_string(),
        Scalar::Number(number) => number.to_string(),
        Scalar::Other => "null".to_string(),
    }
}

fn text_of(value: &Option<AxValue>) -> String {
    match value.as_ref().and_then(|value| value.value.as_ref()) {
        Some(Scalar::Text(text)) => text.clone(),
        _ => String::new(),
    }
}

impl Run<'_> {
    /// The page as text: its address and title, then its accessibility tree, one line per node, each with a ref the
    /// other tools take. `target` narrows it to one element's subtree. A page dialog is reported instead of a tree.
    pub async fn snapshot(
        &self,
        target: Option<&str>,
        depth: Option<u32>,
        boxes: bool,
    ) -> Result<String, ToolError> {
        if let Some(note) = self.dialog_note()? {
            return Ok(note);
        }
        self.refresh()?;
        let limit = depth.unwrap_or(12);
        let tree = self
            .cdp::<AccessibilityGetFullAxTree>(&GetFullAxTreeParams {
                depth: (limit + 2).min(30),
            })
            .await;
        let tree = match tree {
            Ok(tree) => tree,
            // The page opened a dialog while the call waited (an action just ended, and its script was still busy): the
            // dialog is the page's state now, and what this snapshot reports.
            Err(error) => {
                return match self.dialog_note() {
                    Ok(Some(note)) if self.ctx.guard().is_ok() => Ok(note),
                    _ => Err(error),
                };
            }
        };
        let nodes = tree.nodes;
        let by_id: HashMap<&str, &AxNode> = nodes
            .iter()
            .map(|node| (node.node_id.as_str(), node))
            .collect();
        let mut root = nodes.first();
        if let Some(target) = target {
            let object = self.element(target).await?;
            let described = self.backend_node(&object).await?;
            root = described.and_then(|id| {
                nodes
                    .iter()
                    .find(|node| node.backend_dom_node_id == Some(id))
            });
            if root.is_none() {
                return Err(ToolError::new("The target has no accessibility snapshot."));
            }
        }
        let url = self.tab.source()?;
        let title = self.tab.title()?;
        let mut lines = vec![
            format!("URL: {}", json(&bounded(&url, 8192))),
            format!("Title: {}", json(&bounded(&title, 2000))),
        ];
        let mut seen: HashSet<&str> = HashSet::new();
        let mut length = 0usize;
        let mut stack: Vec<(&AxNode, u32)> = root.map(|node| vec![(node, 0)]).unwrap_or_default();
        while let Some((node, level)) = stack.pop() {
            if seen.len() >= NODE_LIMIT
                || length >= TEXT_LIMIT - 3000
                || level > limit
                || seen.contains(node.node_id.as_str())
            {
                continue;
            }
            seen.insert(node.node_id.as_str());
            if !node.ignored {
                let reference = self.reference(node);
                let role = Some(text_of(&node.role))
                    .filter(|role| !role.is_empty())
                    .unwrap_or_else(|| "node".into());
                let mut line = format!(
                    "{}- {} {}",
                    "  ".repeat(level as usize),
                    role,
                    json(&bounded(&text_of(&node.name), 1000))
                );
                if !reference.is_empty() {
                    line.push_str(&format!(" [ref={reference}]"));
                }
                for property in &node.properties {
                    if SHOWN.contains(&property.name.as_str()) {
                        let shown = property
                            .value
                            .value
                            .as_ref()
                            .map_or_else(|| "undefined".to_string(), js_text);
                        line.push_str(&format!(" [{}={shown}]", property.name));
                    }
                }
                let protected = node.properties.iter().any(|property| {
                    property.name == "protected" && property.value.value == Some(Scalar::Bool(true))
                });
                if let (false, Some(value)) = (
                    protected,
                    node.value.as_ref().and_then(|value| value.value.as_ref()),
                ) {
                    line.push_str(&format!(
                        " [value={}]",
                        json(&bounded(&js_text(value), 1000))
                    ));
                }
                if boxes
                    && !reference.is_empty()
                    && seen.len() <= BOX_LIMIT
                    && let Ok(object) = self.element(&reference).await
                    // Detached or layout-less accessibility nodes have no box.
                    && let Ok(rect) = self.rect(&object).await
                {
                    let [x, y, width, height] =
                        [rect.x, rect.y, rect.width, rect.height].map(js_round);
                    line.push_str(&format!(" [box={x},{y},{width},{height}]"));
                }
                length += line.chars().count();
                lines.push(line);
            }
            let next = level + u32::from(!node.ignored);
            for id in node.child_ids.iter().rev() {
                if let Some(child) = by_id.get(id.as_str()) {
                    stack.push((child, next));
                }
            }
        }
        if seen.len() < nodes.len() {
            lines.push(
                "[Snapshot bounded by requested depth, node count or output size. Use a target or browser_find to narrow it.]"
                    .to_string(),
            );
        }
        Ok(bounded(&lines.join("\n"), TEXT_LIMIT))
    }

    /// What a snapshot says while the page is blocked on a dialog, if it is.
    fn dialog_note(&self) -> Result<Option<String>, ToolError> {
        let view = self.ctx.engine.tool_view(&self.bot.id)?;
        Ok(view.dialog.map(|dialog| {
            format!(
                "Page dialog ({}): {}\nUse browser_handle_dialog before continuing.",
                dialog.kind,
                bounded(&dialog.message, 4000)
            )
        }))
    }

    /// The ref of a node: the one it already has in this document, or the next. Empty for a node with no backing element.
    fn reference(&self, node: &AxNode) -> String {
        let Some(backend) = node.backend_dom_node_id.filter(|id| *id != 0) else {
            return String::new();
        };
        let mut state = self.page.state.borrow_mut();
        let reference = match state.reverse.get(&backend) {
            Some(known) => known.clone(),
            None => {
                state.next_ref += 1;
                format!("n{}_{}", state.document, state.next_ref)
            }
        };
        state.reverse.insert(backend, reference.clone());
        state.refs.insert(reference.clone(), backend);
        reference
    }

    /// Lines of the snapshot that match plain text (any case) or a simple regular expression, each with the line before
    /// and after it.
    pub async fn find(&self, text: Option<&str>, regex: Option<&str>) -> Result<String, ToolError> {
        if text.is_some() == regex.is_some() {
            return Err(ToolError::new("Supply text or regex, not both."));
        }
        let snapshot = self.snapshot(None, None, false).await?;
        let lines: Vec<&str> = snapshot.split('\n').collect();
        let pattern = regex.map(simple_regex).transpose()?;
        let lowered = text.map(str::to_lowercase);
        let mut hits = BTreeSet::new();
        for (index, line) in lines.iter().enumerate() {
            let matched = match (&pattern, &lowered) {
                (Some(pattern), _) => pattern.is_match(line),
                (None, Some(text)) => line.to_lowercase().contains(text.as_str()),
                (None, None) => false,
            };
            if matched {
                hits.extend(
                    index.saturating_sub(1)..=(index + 1).min(lines.len().saturating_sub(1)),
                );
            }
        }
        let found: Vec<&str> = hits
            .iter()
            .take(100)
            .filter_map(|index| lines.get(*index).copied())
            .collect();
        Ok(if found.is_empty() {
            "No matches in the bounded snapshot.".to_string()
        } else {
            found.join("\n")
        })
    }
}
