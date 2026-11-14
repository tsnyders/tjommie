//! The browser tools' entry point (`nativeBrowserAutomation` in `native-browser-automation.ts`): one call at a time per
//! bot's tool turn, each with its own 22-second context, cancelled by `cancel`, and answering in words, never by failing.
//! Tools that act on the page run in an [`AutomationSession`](crate::browser::engine::AutomationSession); the eight that
//! do not (navigation, tabs, dialogs, resize) go through the engine's own commands.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use protocol::browser::{HistoryAction, TabAction as UiTabAction};
use protocol::native_browser::{NativeBrowserToolCommand, NativeBrowserUiCommand};

use super::args::{self, TabAction, TabsArgs, ToolCall};
use super::page::Page;
use super::protocol::{
    DomEnable, NetworkEnable, NetworkEnableParams, PageEnable, PageSetInterceptFileChooserDialog,
    ReleaseObjectGroupParams, RuntimeDiscardConsoleEntries, RuntimeEnable,
    RuntimeReleaseObjectGroup, SetInterceptFileChooserParams,
};
use super::reply::{IMAGE_LIMIT, TEXT_LIMIT, ToolError, ToolReply};
use super::run::{GROUP, Run};
use super::session::{ACTION, Context};
use crate::browser::BotIdentity;
use crate::browser::cdp::NoParams;
use crate::browser::engine::{Actor, Engine, EngineError, TabHandle};
use protocol::browser::BrowserState;

/// Who the bot's own commands are, as far as a browser state's `can_control` goes.
const OWNER: &str = "bot";

/// The browser tools of one engine.
pub struct Automation {
    engine: Rc<Engine>,
    /// What each bot's tabs remember between calls, dropped with the tab.
    pages: RefCell<HashMap<String, HashMap<String, Rc<Page>>>>,
    /// The calls running now, by bot: each one's cancellation flag.
    running: RefCell<HashMap<String, Vec<Rc<Cell<bool>>>>>,
}

/// A running call in `Automation::running` for as long as it lives.
struct Registered<'a> {
    automation: &'a Automation,
    bot: String,
    flag: Rc<Cell<bool>>,
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        if let Ok(mut running) = self.automation.running.try_borrow_mut()
            && let Some(flags) = running.get_mut(&self.bot)
        {
            flags.retain(|flag| !Rc::ptr_eq(flag, &self.flag));
            if flags.is_empty() {
                running.remove(&self.bot);
            }
        }
    }
}

fn refusal(error: ToolError) -> EngineError {
    EngineError::refused(&error.0)
}

fn json(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

impl Automation {
    pub fn new(engine: Rc<Engine>) -> Rc<Self> {
        Rc::new(Self {
            engine,
            pages: RefCell::new(HashMap::new()),
            running: RefCell::new(HashMap::new()),
        })
    }

    /// Runs one tool for a bot and answers as the model reads it. A tool that fails answers with its reason and the error
    /// flag; one that was cancelled or overtaken by a private interaction answers that its result was discarded.
    pub async fn run(
        self: &Rc<Self>,
        bot: &BotIdentity,
        command: &NativeBrowserToolCommand,
    ) -> ToolReply {
        let flag = Rc::new(Cell::new(false));
        self.running
            .borrow_mut()
            .entry(bot.id.clone())
            .or_default()
            .push(flag.clone());
        let _registered = Registered {
            automation: self,
            bot: bot.id.clone(),
            flag: flag.clone(),
        };
        let ctx = Context::new(self.engine.clone(), bot.clone(), flag);
        match self.execute(&ctx, command).await {
            Ok(reply) => reply,
            Err(error) => {
                let hidden = ctx.cancelled.get()
                    || match self.engine.tool_view(&bot.id) {
                        Ok(view) => {
                            view.private_active
                                || ctx
                                    .private_epoch
                                    .get()
                                    .is_some_and(|epoch| epoch != view.private_epoch)
                        }
                        Err(_) => true,
                    };
                ToolReply::error(if hidden {
                    "The browser tool was cancelled; its result was discarded."
                } else {
                    &error.0
                })
            }
        }
    }

    /// Cancels the bot's running tools: each stops before its next step, and a file chooser the bot had opened is let go.
    pub fn cancel(&self, bot_id: &str) {
        if let Some(flags) = self.running.borrow().get(bot_id) {
            for flag in flags {
                flag.set(true);
            }
        }
        if let Some(pages) = self.pages.borrow().get(bot_id) {
            for page in pages.values() {
                page.state.borrow_mut().file_input = None;
            }
        }
    }

    async fn execute(
        self: &Rc<Self>,
        ctx: &Context,
        command: &NativeBrowserToolCommand,
    ) -> Result<ToolReply, ToolError> {
        self.engine.open(&ctx.bot).await?;
        let view = self.engine.tool_view(&ctx.bot.id)?;
        ctx.control_epoch.set(Some(view.control_epoch));
        ctx.private_epoch.set(Some(view.private_epoch));
        ctx.guard()?;
        let call = args::decode(&command.name, command.args.as_map()).map_err(ToolError)?;
        if !call.needs_page() {
            let reply = self.off_page(ctx, call).await?;
            ctx.guard()?;
            return Ok(reply);
        }
        self.in_page(ctx, async |run| {
            let reply = run.dispatch(call).await?;
            run.ctx.guard()?;
            Ok(reply)
        })
        .await
    }

    /// Runs `body` on the bot's active page, holding the bot's turn, the page awake and an access ticket for as long as it
    /// takes. The call's page objects are released afterwards, and a result is refused if control changed meanwhile.
    async fn in_page<T>(
        &self,
        ctx: &Context,
        body: impl AsyncFnOnce(&Run<'_>) -> Result<T, ToolError>,
    ) -> Result<T, ToolError> {
        let session = self.engine.automation(&ctx.bot).await?;
        let tab = session.tab().clone();
        let outcome = match self.page_for(&ctx.bot.id, &tab) {
            Err(error) => Err(error),
            Ok(page) => {
                let run = Run {
                    ctx,
                    tab: tab.clone(),
                    page,
                    bot: &ctx.bot,
                };
                let outcome = match self.prepare(&run).await {
                    Ok(()) => body(&run).await,
                    Err(error) => Err(error),
                };
                ctx.cleanup::<RuntimeReleaseObjectGroup>(
                    &tab,
                    &ReleaseObjectGroupParams {
                        object_group: GROUP.to_string(),
                    },
                )
                .await;
                if ctx.cancelled.get() {
                    ctx.cleanup::<PageSetInterceptFileChooserDialog>(
                        &tab,
                        &SetInterceptFileChooserParams { enabled: false },
                    )
                    .await;
                }
                outcome
            }
        };
        let ended = session.finish();
        let value = outcome?;
        ended?;
        Ok(value)
    }

    /// What the tab remembers between calls, made (and watched) the first time a tool needs the tab.
    fn page_for(&self, bot_id: &str, tab: &TabHandle) -> Result<Rc<Page>, ToolError> {
        let live = self.engine.tab_ids(bot_id);
        let known = {
            let mut pages = self.pages.borrow_mut();
            let mine = pages.entry(bot_id.to_string()).or_default();
            mine.retain(|id, _| live.iter().any(|alive| alive == id));
            mine.get(tab.id()).cloned()
        };
        if let Some(page) = known {
            return Ok(page);
        }
        let page = Page::attach(tab)?;
        self.pages
            .borrow_mut()
            .entry(bot_id.to_string())
            .or_default()
            .insert(tab.id().to_string(), page.clone());
        Ok(page)
    }

    /// Notes who had the browser, enables the page's DevTools domains once, and has file choosers report to the tool.
    async fn prepare(&self, run: &Run<'_>) -> Result<(), ToolError> {
        let view = self.engine.tool_view(&run.bot.id)?;
        if run.ctx.viewport_revision.get().is_none() {
            run.ctx.viewport_revision.set(Some(view.viewport_revision));
        }
        run.page.sync((view.control_epoch, view.private_epoch));
        if view.dialog.is_some() {
            // A page blocked on a dialog answers no DevTools command: nothing can be enabled now, and the snapshot (which
            // needs none) reports the dialog. The next call, after the dialog is answered, prepares the page.
            return Ok(());
        }
        if !run.page.ready.get() {
            // Chromium replays retained console calls when Runtime is enabled again after a private interaction.
            run.cdp::<RuntimeDiscardConsoleEntries>(&NoParams {})
                .await?;
            run.cdp::<RuntimeEnable>(&NoParams {}).await?;
            run.cdp::<DomEnable>(&NoParams {}).await?;
            run.cdp::<PageEnable>(&NoParams {}).await?;
            run.cdp::<NetworkEnable>(&NetworkEnableParams {
                max_total_buffer_size: u32::try_from(IMAGE_LIMIT).unwrap_or(u32::MAX),
                max_resource_buffer_size: u32::try_from(TEXT_LIMIT).unwrap_or(u32::MAX),
                max_post_data_size: 0,
            })
            .await?;
            run.page.ready.set(true);
        }
        // Bot-opened choosers may be delayed by page script, so they stay intercepted for as long as the bot has the
        // browser; the engine gives the native picker back to the person.
        run.cdp::<PageSetInterceptFileChooserDialog>(&SetInterceptFileChooserParams {
            enabled: true,
        })
        .await?;
        run.tab.note_file_chooser_interception(true);
        Ok(())
    }

    // ---- the tools that do not run on the page ---------------------------------------------------------------

    async fn off_page(&self, ctx: &Context, call: ToolCall) -> Result<ToolReply, ToolError> {
        match call {
            ToolCall::Resize(args) => {
                self.engine.open(&ctx.bot).await?;
                let check = || ctx.guard().map_err(refusal);
                self.engine
                    .resize(&ctx.bot, args.width, args.height, Some(&check))
                    .await?;
                self.tab_list(ctx)
            }
            ToolCall::Navigate(args) => {
                let (generation, page_id) = self.target(ctx).await?;
                self.act(
                    ctx,
                    NativeBrowserUiCommand::Navigate {
                        generation,
                        page_id,
                        url: args.url,
                    },
                )
                .await?;
                self.after_navigation(ctx).await
            }
            ToolCall::NavigateBack => self.history(ctx, HistoryAction::Back).await,
            ToolCall::NavigateForward => self.history(ctx, HistoryAction::Forward).await,
            ToolCall::Reload => self.history(ctx, HistoryAction::Reload).await,
            ToolCall::Close => {
                let (generation, page_id) = self.target(ctx).await?;
                self.act(
                    ctx,
                    NativeBrowserUiCommand::Tab {
                        generation,
                        action: UiTabAction::Close,
                        page_id: Some(page_id),
                    },
                )
                .await?;
                self.tab_list(ctx)
            }
            ToolCall::Tabs(args) => self.tabs(ctx, &args).await,
            ToolCall::HandleDialog(args) => {
                self.engine.open(&ctx.bot).await?;
                let state = self.engine.state(&ctx.bot.id, OWNER)?;
                ctx.guard()?;
                let dialog = state
                    .dialog
                    .ok_or_else(|| ToolError::new("No page dialog is open."))?;
                self.act(
                    ctx,
                    NativeBrowserUiCommand::Dialog {
                        generation: state.generation,
                        page_id: dialog.page_id,
                        accept: args.accept,
                        prompt_text: args.prompt_text,
                    },
                )
                .await?;
                self.tab_list(ctx)
            }
            _ => Err(ToolError::new("That tool runs on the page.")),
        }
    }

    /// An engine command as the bot: refused the moment the call's authority goes.
    async fn act(
        &self,
        ctx: &Context,
        command: NativeBrowserUiCommand,
    ) -> Result<BrowserState, ToolError> {
        let check = || ctx.guard().map_err(refusal);
        Ok(self
            .engine
            .act(&ctx.bot, &command, Actor::Bot, Some(&check), OWNER)
            .await?)
    }

    /// The bot's browser generation and active tab: what a navigation or tab command is aimed at.
    async fn target(&self, ctx: &Context) -> Result<(String, String), ToolError> {
        self.engine.open(&ctx.bot).await?;
        let state = self.engine.state(&ctx.bot.id, OWNER)?;
        ctx.guard()?;
        let page = state
            .active_page_id
            .ok_or_else(|| ToolError::new("No browser tab is open."))?;
        Ok((state.generation, page))
    }

    async fn history(&self, ctx: &Context, action: HistoryAction) -> Result<ToolReply, ToolError> {
        let (generation, page_id) = self.target(ctx).await?;
        self.act(
            ctx,
            NativeBrowserUiCommand::History {
                generation,
                page_id,
                action,
            },
        )
        .await?;
        self.after_navigation(ctx).await
    }

    /// The tabs, one per line, the current one marked.
    fn tab_list(&self, ctx: &Context) -> Result<ToolReply, ToolError> {
        let state = self.engine.state(&ctx.bot.id, OWNER)?;
        let lines: Vec<String> = state
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let current = if state.active_page_id.as_deref() == Some(tab.id.as_str()) {
                    "(current) "
                } else {
                    ""
                };
                format!("- {index}: {current}{} ({})", json(&tab.title), tab.url)
            })
            .collect();
        Ok(ToolReply::text(&if lines.is_empty() {
            "No open tabs.".to_string()
        } else {
            lines.join("\n")
        }))
    }

    async fn tabs(&self, ctx: &Context, args: &TabsArgs) -> Result<ToolReply, ToolError> {
        self.engine.open(&ctx.bot).await?;
        let mut state = self.engine.state(&ctx.bot.id, OWNER)?;
        ctx.guard()?;
        let action = match args.action {
            TabAction::List => return self.tab_list(ctx),
            TabAction::New => UiTabAction::New,
            TabAction::Close => UiTabAction::Close,
            TabAction::Select => UiTabAction::Select,
        };
        let selected = match args.index {
            None => state.active_page_id.clone(),
            Some(index) => state.tabs.get(index as usize).map(|tab| tab.id.clone()),
        };
        if (args.index.is_some() && selected.is_none())
            || (args.action == TabAction::Select && args.index.is_none())
        {
            return Err(ToolError::new("Choose an existing tab index."));
        }
        state = self
            .act(
                ctx,
                NativeBrowserUiCommand::Tab {
                    generation: state.generation.clone(),
                    action,
                    page_id: selected,
                },
            )
            .await?;
        if let (TabAction::New, Some(url), Some(page_id)) =
            (args.action, &args.url, &state.active_page_id)
        {
            self.act(
                ctx,
                NativeBrowserUiCommand::Navigate {
                    generation: state.generation.clone(),
                    page_id: page_id.clone(),
                    url: url.clone(),
                },
            )
            .await?;
            return self.after_navigation(ctx).await;
        }
        self.tab_list(ctx)
    }

    /// What a navigation answers with: the page's snapshot once it has loaded (or, after ten seconds, a note that it has not).
    async fn after_navigation(&self, ctx: &Context) -> Result<ToolReply, ToolError> {
        self.in_page(ctx, async |run| {
            run.ctx.delay(60).await?;
            let limit = run
                .ctx
                .deadline
                .checked_sub(Duration::from_secs(1))
                .unwrap_or(run.ctx.deadline);
            let until = (Instant::now() + ACTION).min(limit);
            while run.tab.is_loading() && Instant::now() < until && !run.ctx.dialog_open() {
                run.ctx.delay(50).await?;
            }
            let loading = if run.tab.is_loading() {
                "Navigation is still loading.\n"
            } else {
                ""
            };
            let snapshot = run.snapshot(None, None, false).await?;
            Ok(ToolReply::text(&format!("{loading}{snapshot}")))
        })
        .await
    }
}
