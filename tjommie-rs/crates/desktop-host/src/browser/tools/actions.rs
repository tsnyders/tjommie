//! The tools that act on the page: click, hover, type, select, fill a form, drag, drop, press a key, and run JavaScript
//! (`nativeBrowserTools` in `native-browser-automation.ts`), and the dispatcher that picks the tool for a decoded call.

use std::path::{Path, PathBuf};

use super::args::{
    Button, ClickArgs, DragArgs, DropArgs, EvaluateArgs, FieldType, FillFormArgs, Modifier,
    SelectOptionArgs, TargetArgs, ToolCall, TypeArgs,
};
use super::files;
use super::protocol::{
    ArgValue, DispatchDragParams, DragData, DragEventType, DragItem, InputDispatchDragEvent, Scalar,
};
use super::reply::{TEXT_LIMIT, ToolError, ToolReply, bounded};
use super::run::{Run, by_value, js_round, left_button, mouse_event, target_of};
use crate::browser::cdp::MouseEventType;

const SLIDER: &str = "function(value) { if (!(this instanceof HTMLInputElement) || this.type !== 'range' || !Number.isFinite(Number(value))) throw new Error('A slider requires a range input and numeric value.'); const min = Number(this.min || 0), max = Number(this.max || 100); if (Number(value) < min || Number(value) > max) throw new Error('Slider value is outside its range.'); this.value = value; this.dispatchEvent(new Event('input',{bubbles:true})); this.dispatchEvent(new Event('change',{bubbles:true})); }";

fn modifier_bit(modifier: Modifier) -> u32 {
    match modifier {
        Modifier::Alt => 1,
        Modifier::Control | Modifier::ControlOrMeta => 2,
        Modifier::Meta => 4,
        Modifier::Shift => 8,
    }
}

/// The element a call names, if it names one: a target, a ref, or both if they agree.
fn named<'t>(
    target: &'t Option<String>,
    reference: &'t Option<String>,
) -> Result<Option<&'t str>, ToolError> {
    if target.is_none() && reference.is_none() {
        return Ok(None);
    }
    target_of(target.as_deref(), reference.as_deref()).map(Some)
}

impl Run<'_> {
    /// Runs the tool a decoded call names, on this page.
    pub async fn dispatch(&self, call: ToolCall) -> Result<ToolReply, ToolError> {
        match call {
            ToolCall::Snapshot(args) => {
                let target = named(&args.target, &args.reference)?;
                let text = self
                    .snapshot(target, args.depth, args.boxes.unwrap_or(false))
                    .await?;
                self.result(&text, args.filename.as_deref())
            }
            ToolCall::Click(args) => self.click_tool(&args).await,
            ToolCall::Hover(args) => self.hover_tool(&args).await,
            ToolCall::Type(args) => self.type_tool(&args).await,
            ToolCall::SelectOption(args) => self.select_tool(&args).await,
            ToolCall::FillForm(args) => self.fill_form_tool(&args).await,
            ToolCall::Drag(args) => self.drag_tool(&args).await,
            ToolCall::Drop(args) => self.drop_tool(&args).await,
            ToolCall::EmulateMedia(args) => self.emulate_media_tool(&args).await,
            ToolCall::TakeScreenshot(args) => self.screenshot_tool(&args).await,
            ToolCall::PressKey(args) => {
                self.key(&args.key).await?;
                self.ctx
                    .engine
                    .sleep(std::time::Duration::from_millis(20))
                    .await;
                self.shown().await
            }
            ToolCall::Evaluate(args) => self.evaluate_tool(&args).await,
            ToolCall::ConsoleMessages(args) => self.console_tool(&args),
            ToolCall::NetworkRequests(args) => self.requests_tool(&args),
            ToolCall::NetworkRequest(args) => self.request_tool(&args).await,
            ToolCall::FileUpload(args) => self.upload_tool(&args).await,
            ToolCall::WaitFor(args) => self.wait_tool(&args).await,
            ToolCall::Find(args) => Ok(ToolReply::text(
                &self
                    .find(args.text.as_deref(), args.regex.as_deref())
                    .await?,
            )),
            ToolCall::Resize(_)
            | ToolCall::Navigate(_)
            | ToolCall::NavigateBack
            | ToolCall::NavigateForward
            | ToolCall::Reload
            | ToolCall::Close
            | ToolCall::Tabs(_)
            | ToolCall::HandleDialog(_) => {
                Err(ToolError::new("That tool does not run on the page."))
            }
        }
    }

    /// The snapshot every acting tool answers with.
    pub async fn shown(&self) -> Result<ToolReply, ToolError> {
        Ok(ToolReply::text(&self.snapshot(None, None, false).await?))
    }

    /// `text` as the answer, or saved in the bot's `browser-files` folder if the call named a file.
    pub fn result(&self, text: &str, filename: Option<&str>) -> Result<ToolReply, ToolError> {
        match filename {
            None => Ok(ToolReply::text(text)),
            Some(name) => {
                let path = self.save(bounded(text, TEXT_LIMIT).as_bytes(), name)?;
                Ok(ToolReply::text(&format!(
                    "Saved browser output: {}",
                    path.display()
                )))
            }
        }
    }

    /// Saves `data` as a new file in the bot's `browser-files` folder, if the call is still the bot's.
    pub fn save(&self, data: &[u8], filename: &str) -> Result<PathBuf, ToolError> {
        self.ctx.guard()?;
        let path = files::write_output(Path::new(&self.bot.home_folder), filename, data)?;
        self.ctx.guard()?;
        Ok(path)
    }

    /// The files a call may send to the page: regular files in the bot's home or project folder, at most 20 MiB together.
    pub fn uploads(&self, paths: &[String]) -> Result<Vec<String>, ToolError> {
        files::uploads(
            Path::new(&self.bot.home_folder),
            self.bot.project_folder.as_deref().map(Path::new),
            paths,
        )
    }

    async fn click_tool(&self, args: &ClickArgs) -> Result<ToolReply, ToolError> {
        let target = target_of(args.target.as_deref(), args.reference.as_deref())?;
        let modifiers = args
            .modifiers
            .iter()
            .flatten()
            .fold(0, |held, modifier| held | modifier_bit(*modifier));
        let times = if args.double_click == Some(true) {
            2
        } else {
            1
        };
        self.click(
            target,
            args.button.unwrap_or(Button::Left),
            times,
            modifiers,
        )
        .await?;
        self.shown().await
    }

    async fn hover_tool(&self, args: &TargetArgs) -> Result<ToolReply, ToolError> {
        let point = self
            .actionable(
                target_of(args.target.as_deref(), args.reference.as_deref())?,
                false,
            )
            .await?;
        self.mouse(mouse_event(
            MouseEventType::MouseMoved,
            js_round(point.x) as f64,
            js_round(point.y) as f64,
        ))
        .await?;
        self.shown().await
    }

    async fn type_tool(&self, args: &TypeArgs) -> Result<ToolReply, ToolError> {
        let slowly = args.slowly == Some(true);
        if slowly && args.text.encode_utf16().count() > 2000 {
            return Err(ToolError::new("Slow typing is limited to 2000 characters."));
        }
        let target = target_of(args.target.as_deref(), args.reference.as_deref())?;
        self.fill(target, &args.text, slowly).await?;
        if args.submit == Some(true) {
            self.key("Enter").await?;
        }
        self.shown().await
    }

    async fn select_tool(&self, args: &SelectOptionArgs) -> Result<ToolReply, ToolError> {
        let target = target_of(args.target.as_deref(), args.reference.as_deref())?;
        self.select(target, &args.values, false).await?;
        self.shown().await
    }

    async fn fill_form_tool(&self, args: &FillFormArgs) -> Result<ToolReply, ToolError> {
        for field in &args.fields {
            let target = target_of(field.target.as_deref(), field.reference.as_deref())?;
            match field.field_type {
                FieldType::Textbox => self.fill(target, &field.value, false).await?,
                FieldType::Combobox => {
                    self.select(target, std::slice::from_ref(&field.value), true)
                        .await?
                }
                FieldType::Checkbox | FieldType::Radio => {
                    if field.value != "true" && field.value != "false" {
                        return Err(ToolError::new(
                            "Checkbox and radio values must be true or false.",
                        ));
                    }
                    self.check(target, field.value == "true").await?;
                }
                FieldType::Slider => {
                    let item = self.actionable(target, false).await?;
                    self.call::<Scalar>(
                        &item.object_id,
                        SLIDER,
                        vec![by_value(ArgValue::Text(field.value.clone()))],
                    )
                    .await?;
                }
            }
        }
        self.shown().await
    }

    async fn drag_tool(&self, args: &DragArgs) -> Result<ToolReply, ToolError> {
        let from = self
            .actionable(
                target_of(args.start_target.as_deref(), args.start_ref.as_deref())?,
                false,
            )
            .await?;
        let to = self
            .actionable(
                target_of(args.end_target.as_deref(), args.end_ref.as_deref())?,
                false,
            )
            .await?;
        self.ctx.guard()?;
        // The second element's scroll may have moved the first: measure both again.
        let (start, finish) = (
            self.rect(&from.object_id).await?,
            self.rect(&to.object_id).await?,
        );
        let (from_x, from_y) = (start.x + start.width / 2.0, start.y + start.height / 2.0);
        let (to_x, to_y) = (
            finish.x + finish.width / 2.0,
            finish.y + finish.height / 2.0,
        );
        let (mut x, mut y) = (js_round(from_x) as f64, js_round(from_y) as f64);
        self.ctx.guard()?;
        self.mouse(mouse_event(MouseEventType::MouseMoved, x, y))
            .await?;
        let pressed = self
            .mouse(left_button(MouseEventType::MousePressed, x, y))
            .await;
        let dragged = self
            .drag_steps(&mut x, &mut y, (from_x, from_y), (to_x, to_y))
            .await;
        self.release(left_button(MouseEventType::MouseReleased, x, y))
            .await;
        pressed?;
        dragged?;
        self.shown().await
    }

    /// Ten moves from one point to the other with the left button held, 15 ms apart.
    async fn drag_steps(
        &self,
        x: &mut f64,
        y: &mut f64,
        from: (f64, f64),
        to: (f64, f64),
    ) -> Result<(), ToolError> {
        for step in 1..=10 {
            self.ctx.delay(15).await?;
            let fraction = f64::from(step) / 10.0;
            *x = js_round(from.0 + (to.0 - from.0) * fraction) as f64;
            *y = js_round(from.1 + (to.1 - from.1) * fraction) as f64;
            self.mouse(left_button(MouseEventType::MouseMoved, *x, *y))
                .await?;
        }
        Ok(())
    }

    async fn drop_tool(&self, args: &DropArgs) -> Result<ToolReply, ToolError> {
        let empty = std::collections::BTreeMap::new();
        let data = args.data.as_ref().unwrap_or(&empty);
        if args.paths.as_ref().is_none_or(Vec::is_empty) && data.is_empty() {
            return Err(ToolError::new("Supply paths or MIME data."));
        }
        if data.len() > 20 {
            return Err(ToolError::new("At most 20 MIME data items are supported."));
        }
        let files = match &args.paths {
            Some(paths) => self.uploads(paths)?,
            None => Vec::new(),
        };
        let point = self
            .actionable(
                target_of(args.target.as_deref(), args.reference.as_deref())?,
                false,
            )
            .await?;
        let carried = DragData {
            items: data
                .iter()
                .map(|(mime_type, data)| DragItem {
                    mime_type: mime_type.clone(),
                    data: data.clone(),
                })
                .collect(),
            files,
            drag_operations_mask: 1,
        };
        for kind in [
            DragEventType::DragEnter,
            DragEventType::DragOver,
            DragEventType::Drop,
        ] {
            self.cdp::<InputDispatchDragEvent>(&DispatchDragParams {
                kind,
                x: point.x,
                y: point.y,
                data: carried.clone(),
            })
            .await?;
        }
        self.shown().await
    }

    async fn evaluate_tool(&self, args: &EvaluateArgs) -> Result<ToolReply, ToolError> {
        let serialize = format!(
            "const serialized = JSON.stringify(value ?? null, null, 2); return serialized.length > {TEXT_LIMIT} ? serialized.slice(0, {TEXT_LIMIT}) + '\\n[truncated]' : serialized;"
        );
        let function = &args.function;
        let value: Option<Scalar> = if args.target.is_some() || args.reference.is_some() {
            let object = self
                .element(target_of(
                    args.target.as_deref(),
                    args.reference.as_deref(),
                )?)
                .await?;
            let declaration = format!(
                "async function() {{ const fn = ({function}); const value = typeof fn === 'function' ? await fn(this) : fn; {serialize} }}"
            );
            self.call(&object, &declaration, Vec::new()).await?
        } else {
            let expression = format!(
                "(async () => {{ const fn = ({function}); const value = typeof fn === 'function' ? await fn() : fn; {serialize} }})()"
            );
            self.eval(&expression).await?
        };
        let text = match value {
            Some(Scalar::Text(text)) => text,
            Some(Scalar::Bool(flag)) => flag.to_string(),
            Some(Scalar::Number(number)) => number.to_string(),
            Some(Scalar::Other) | None => "null".to_string(),
        };
        self.result(&text, args.filename.as_deref())
    }
}
