import { randomUUID } from "node:crypto";
import type { EventEmitter } from "node:events";
import { lstat, mkdir, realpath, stat, writeFile } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import type { WebContents, MouseInputEvent } from "electron";
import { z } from "zod";
import type { NativeBrowserToolCommand, NativeBrowserUiCommand } from "../../protocol/src/native-browser.ts";
import type { NativeBrowserEngine, NativeBrowserBot } from "./native-browser.ts";

const TEXT_LIMIT = 128 * 1024, IMAGE_LIMIT = 2 * 1024 * 1024, ACTION_MS = 10_000;
const record = (value: unknown): Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};
const string = (value: unknown): string => typeof value === "string" ? value : "";
const bounded = (value: string, limit = TEXT_LIMIT): string => value.length > limit ? `${value.slice(0, limit)}\n[truncated]` : value;
const pause = (ms: number): Promise<void> => new Promise((done) => setTimeout(done, ms));
export type NativeBrowserToolResult = { content: ({ type: "text"; text: string } | { type: "image"; data: string; mimeType: "image/png" | "image/jpeg" | "image/webp" })[]; isError?: boolean };
const text = (value: string): NativeBrowserToolResult => ({ content: [{ type: "text", text: bounded(value) }] });
type Observation = { level: string; text: string; document: string };
type Request = { id: string; index: number; url: string; method: string; type: string; status?: number; requestHeaders: Record<string, unknown>; responseHeaders?: Record<string, unknown> };
type PageState = { document: string; nextRef: number; refs: Map<string, number>; reverse: Map<number, string>; console: Observation[]; requests: Map<string, Request>; nextRequest: number; fileInput?: number; ready?: Promise<void>; media: Map<string, string>; mediaType?: string };
const pages = new WeakMap<WebContents, PageState>();
type Context = { engine: NativeBrowserEngine; bot: NativeBrowserBot; contents?: WebContents; page?: PageState; deadline: number; viewportRevision?: number; cancelled?: boolean };
const active = new WeakMap<WebContents, Context>();
const activeBots = new Map<string, Set<Context>>();
class PageDialogError extends Error {}
function guard(ctx: Context): void {
  if (ctx.cancelled) throw new Error("The browser tool was cancelled. Take a fresh snapshot before continuing.");
  const state = ctx.engine.state(ctx.bot.id);
  if (state.control !== "bot") throw new Error("The person has taken browser control. Return control to the bot before continuing.");
  if (ctx.viewportRevision !== undefined && state.viewportRevision !== ctx.viewportRevision) throw new Error("The browser panel resized during this action. Take a fresh snapshot before retrying.");
  if (Date.now() >= ctx.deadline) throw new Error("The browser tool exceeded its 22-second deadline. Take a fresh snapshot before retrying.");
}
async function cleanupCommand(contents: WebContents, method: string, params: Record<string, unknown> = {}): Promise<void> {
  let timer: NodeJS.Timeout | undefined;
  try { await Promise.race([contents.debugger.sendCommand(method, params).catch(() => {}), new Promise<void>((done) => { timer = setTimeout(done, 500); })]); }
  finally { clearTimeout(timer); }
}
export async function cancelNativeBrowserAutomation(botId: string): Promise<void> {
  const contexts = [...(activeBots.get(botId) ?? [])];
  for (const ctx of contexts) { ctx.cancelled = true; if (ctx.page) ctx.page.fileInput = undefined; }
  await Promise.all(contexts.map(async (ctx) => {
    if (ctx.contents) {
      let dialog = true;
      try { dialog = !!ctx.engine.state(botId).dialog; } catch { /* a closing page does not need evaluation termination */ }
      if (!dialog) await cleanupCommand(ctx.contents, "Runtime.terminateExecution");
      await cleanupCommand(ctx.contents, "Page.setInterceptFileChooserDialog", { enabled: false });
    }
  }));
}
async function delay(ctx: Context, ms: number): Promise<void> {
  const end = Date.now() + ms;
  do { guard(ctx); await pause(Math.min(50, Math.max(0, end - Date.now()))); } while (Date.now() < end);
  guard(ctx);
}

function safeHeaders(value: unknown): Record<string, unknown> {
  return Object.fromEntries(Object.entries(record(value)).slice(0, 100).map(([key, value]) => [key, /authorization|cookie|token|secret|api[-_]?key/i.test(key) ? "[redacted]" : bounded(String(value), 2000)]));
}
async function cdp(contents: WebContents, method: string, params: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
  const ctx = active.get(contents);
  if (ctx) guard(ctx);
  let timer: NodeJS.Timeout | undefined, poll: NodeJS.Timeout | undefined;
  let dialogListener: ((_event: unknown, method: string) => void) | undefined;
  try {
    const value = record(await Promise.race([contents.debugger.sendCommand(method, params), new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error("The browser operation timed out. Check for a page dialog, then take a fresh snapshot.")), ACTION_MS);
      if (ctx) poll = setInterval(() => { try { guard(ctx); } catch (error) { reject(error); } }, 50);
      dialogListener = (_event, event) => { if (event === "Page.javascriptDialogOpening") reject(new PageDialogError("The page opened a dialog. Use browser_handle_dialog before continuing.")); };
      contents.debugger.on("message", dialogListener);
    })]));
    if (ctx) guard(ctx);
    return value;
  } catch (error) {
    let dialog = error instanceof PageDialogError;
    try { dialog ||= !!ctx?.engine.state(ctx.bot.id).dialog; } catch { /* preserve the original failure */ }
    if (!dialog && (method === "Runtime.evaluate" || method === "Runtime.callFunctionOn")) {
      // End the current evaluation before releasing the automation queue. Page-created future
      // timers are ordinary page behavior, not a cancellable Playwright/Node execution session.
      await cleanupCommand(contents, "Runtime.terminateExecution");
    }
    throw error;
  } finally { clearTimeout(timer); clearInterval(poll); if (dialogListener) contents.debugger.removeListener("message", dialogListener); }
}
async function prepare(ctx: Context): Promise<void> {
  const contents = ctx.contents!;
  ctx.viewportRevision ??= ctx.engine.state(ctx.bot.id).viewportRevision;
  let page = pages.get(contents);
  if (!page) {
    page = { document: randomUUID().slice(0, 8), nextRef: 0, refs: new Map(), reverse: new Map(), console: [], requests: new Map(), nextRequest: 0, media: new Map() };
    pages.set(contents, page);
    const state = page;
    const reset = (): void => { state.document = randomUUID().slice(0, 8); state.refs.clear(); state.reverse.clear(); state.nextRef = 0; state.fileInput = undefined; state.requests.clear(); state.nextRequest = 0; };
    contents.on("did-navigate", reset);
    contents.on("render-process-gone", reset);
    (contents as unknown as EventEmitter).on("tjommie-browser-control", (control: unknown) => {
      state.fileInput = undefined;
      if (control === "human") void cleanupCommand(contents, "Page.setInterceptFileChooserDialog", { enabled: false });
    });
    contents.once("destroyed", () => { reset(); pages.delete(contents); });
    contents.debugger.on("message", (_event: unknown, method: string, raw: unknown) => {
      const params = record(raw);
      if (method === "DOM.documentUpdated") { reset(); return; }
      // Human sign-in traffic and console output are not retained for later bot inspection.
      try { if (ctx.engine.state(ctx.bot.id).control !== "bot") return; } catch { return; }
      if (method === "Runtime.consoleAPICalled") {
        const values = Array.isArray(params.args) ? params.args : [];
        const message = values.map((value) => { const item = record(value); return string(item.description) || String(item.value ?? ""); }).join(" ");
        state.console.push({ level: string(params.type), text: bounded(message, 4000), document: state.document });
        if (state.console.length > 200) state.console.shift();
      } else if (method === "Runtime.exceptionThrown") {
        state.console.push({ level: "error", text: bounded(string(record(params.exceptionDetails).text), 4000), document: state.document });
        if (state.console.length > 200) state.console.shift();
      } else if (method === "Network.requestWillBeSent") {
        const request = record(params.request), id = string(params.requestId);
        state.requests.set(id, { id, index: ++state.nextRequest, url: bounded(string(request.url), 8192), method: string(request.method), type: string(params.type), requestHeaders: safeHeaders(request.headers) });
        if (state.requests.size > 200) state.requests.delete(state.requests.keys().next().value!);
      } else if (method === "Network.responseReceived") {
        const request = state.requests.get(string(params.requestId)), response = record(params.response);
        if (request) { request.status = typeof response.status === "number" ? response.status : undefined; request.responseHeaders = safeHeaders(response.headers); }
      } else if (method === "Page.fileChooserOpened" && typeof params.backendNodeId === "number") state.fileInput = params.backendNodeId;
    });
  }
  ctx.page = page;
  const state = page;
  state.ready ??= (async () => { await cdp(contents, "Runtime.enable"); await cdp(contents, "DOM.enable"); await cdp(contents, "Network.enable", { maxTotalBufferSize: IMAGE_LIMIT, maxResourceBufferSize: TEXT_LIMIT, maxPostDataSize: 0 }); })().catch((error: unknown) => { state.ready = undefined; throw error; });
  await page.ready;
  // Bot-opened choosers may be delayed by page JavaScript. Keep them intercepted throughout bot
  // control; the main-only handover event restores the native picker for the person.
  await cdp(contents, "Page.setInterceptFileChooserDialog", { enabled: true });
}
function remote(result: Record<string, unknown>): Record<string, unknown> {
  if (result.exceptionDetails) throw new Error(bounded(string(record(record(result.exceptionDetails).exception).description) || string(record(result.exceptionDetails).text) || "Page JavaScript failed.", 2000));
  return record(result.result);
}
async function evaluate(ctx: Context, expression: string): Promise<unknown> {
  return remote(await cdp(ctx.contents!, "Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, timeout: ACTION_MS, userGesture: true })).value;
}
async function call(ctx: Context, objectId: string, declaration: string, values: unknown[] = []): Promise<unknown> {
  return remote(await cdp(ctx.contents!, "Runtime.callFunctionOn", { objectId, functionDeclaration: declaration, arguments: values.map((value) => ({ value })), awaitPromise: true, returnByValue: true, userGesture: true })).value;
}
function target(args: { target?: string; ref?: string }): string {
  if (args.target && args.ref && args.target !== args.ref) throw new Error("Use one target or matching ref, not conflicting targets.");
  const value = args.target ?? args.ref;
  if (!value) throw new Error("An exact snapshot ref or unique CSS target is required.");
  return value;
}
async function element(ctx: Context, value: string): Promise<string> {
  const backend = ctx.page!.refs.get(value);
  if (!backend && /^n[0-9a-f]{8}_\d+$/.test(value)) throw new Error("This element ref is stale or belongs to another tab. Take a fresh snapshot.");
  let object: Record<string, unknown>;
  if (backend) object = record((await cdp(ctx.contents!, "DOM.resolveNode", { backendNodeId: backend, objectGroup: "tjommie-tools" })).object);
  else object = remote(await cdp(ctx.contents!, "Runtime.evaluate", { expression: `(() => { const nodes = document.querySelectorAll(${JSON.stringify(value)}); if (nodes.length !== 1) throw new Error('Target must match exactly one element; take a fresh snapshot.'); return nodes[0]; })()`, objectGroup: "tjommie-tools", returnByValue: false, timeout: ACTION_MS }));
  if (typeof object.objectId !== "string") throw new Error("The element is no longer available. Take a fresh snapshot.");
  return object.objectId;
}
async function box(ctx: Context, objectId: string): Promise<{ x: number; y: number; width: number; height: number }> {
  const raw = await cdp(ctx.contents!, "DOM.getBoxModel", { objectId });
  const quad = record(raw.model).content;
  if (!Array.isArray(quad) || quad.length !== 8 || !quad.every((value) => typeof value === "number" && Number.isFinite(value))) throw new Error("The element has no visible box.");
  const numbers = quad as number[], xs = [numbers[0], numbers[2], numbers[4], numbers[6]], ys = [numbers[1], numbers[3], numbers[5], numbers[7]];
  return { x: Math.min(...xs), y: Math.min(...ys), width: Math.max(...xs) - Math.min(...xs), height: Math.max(...ys) - Math.min(...ys) };
}
async function actionable(ctx: Context, value: string, editable = false): Promise<{ objectId: string; x: number; y: number }> {
  const deadline = Date.now() + 3000;
  let message = "The element is not ready for input.";
  while (Date.now() < deadline) {
    const objectId = await element(ctx, value);
    const readiness = record(await call(ctx, objectId, `function(editable) {
      if (!this.isConnected) return {error:'The element detached; take a fresh snapshot.'};
      if (this.disabled || this.getAttribute('aria-disabled') === 'true' || this.closest('[inert]')) return {error:'The element is disabled.'};
      if (editable && (this.readOnly || !(this instanceof HTMLInputElement || this instanceof HTMLTextAreaElement || this.isContentEditable))) return {error:'The element is not editable.'};
      const style = getComputedStyle(this); const rect = this.getBoundingClientRect();
      if (style.visibility === 'hidden' || style.display === 'none' || !rect.width || !rect.height) return {error:'The element is not visible.'};
      this.scrollIntoView({block:'center',inline:'center',behavior:'instant'});
      return {ok:true};
    }`, [editable]));
    if (readiness.ok) {
      const first = await box(ctx, objectId); await delay(ctx, 30); const last = await box(ctx, objectId);
      if (Math.abs(first.x - last.x) + Math.abs(first.y - last.y) + Math.abs(first.width - last.width) + Math.abs(first.height - last.height) < 1) {
        const x = last.x + last.width / 2, y = last.y + last.height / 2;
        const hit = await cdp(ctx.contents!, "DOM.getNodeForLocation", { x: Math.round(x), y: Math.round(y), includeUserAgentShadowDOM: true });
        if (typeof hit.backendNodeId === "number") {
          const hitObject = record((await cdp(ctx.contents!, "DOM.resolveNode", { backendNodeId: hit.backendNodeId, objectGroup: "tjommie-tools" })).object);
          if (typeof hitObject.objectId === "string") {
            const contains = remote(await cdp(ctx.contents!, "Runtime.callFunctionOn", { objectId, functionDeclaration: "function(hit) { return this === hit || this.contains(hit) || (hit && hit.getRootNode && hit.getRootNode().host === this); }", arguments: [{ objectId: hitObject.objectId }], returnByValue: true })).value;
            if (contains === true) return { objectId, x, y };
          }
        }
        message = "Another element covers the target; input was not sent.";
      } else message = "The element is moving; input was not sent.";
    } else message = string(readiness.error) || message;
    await delay(ctx, 70);
  }
  throw new Error(message);
}
type AxNode = { nodeId: string; ignored?: boolean; role?: { value?: unknown }; name?: { value?: unknown }; value?: { value?: unknown }; backendDOMNodeId?: number; childIds?: string[]; properties?: { name: string; value: { value?: unknown } }[] };
const AxNodes = z.array(z.object({ nodeId: z.string(), ignored: z.boolean().optional(), role: z.object({ value: z.unknown().optional() }).optional(), name: z.object({ value: z.unknown().optional() }).optional(), value: z.object({ value: z.unknown().optional() }).optional(), backendDOMNodeId: z.number().optional(), childIds: z.array(z.string()).optional(), properties: z.array(z.object({ name: z.string(), value: z.object({ value: z.unknown().optional() }) })).optional() }));
async function snapshot(ctx: Context, options: { target?: string; ref?: string; depth?: number; boxes?: boolean } = {}): Promise<string> {
  const dialog = ctx.engine.state(ctx.bot.id).dialog;
  if (dialog) return `Page dialog (${dialog.type}): ${bounded(dialog.message, 4000)}\nUse browser_handle_dialog before continuing.`;
  const raw = await cdp(ctx.contents!, "Accessibility.getFullAXTree", { depth: Math.min(30, (options.depth ?? 12) + 2) });
  const nodes = AxNodes.parse(raw.nodes), map = new Map(nodes.map((node) => [node.nodeId, node]));
  let root: AxNode | undefined = nodes[0];
  if (options.target || options.ref) {
    const objectId = await element(ctx, target(options));
    const described = record((await cdp(ctx.contents!, "DOM.describeNode", { objectId })).node);
    root = nodes.find((node) => node.backendDOMNodeId === described.backendNodeId);
    if (!root) throw new Error("The target has no accessibility snapshot.");
  }
  const lines = [`URL: ${JSON.stringify(bounded(ctx.contents!.getURL(), 8192))}`, `Title: ${JSON.stringify(bounded(ctx.contents!.getTitle(), 2000))}`];
  const seen = new Set<string>(); let length = 0;
  const visit = async (node: AxNode, depth: number): Promise<void> => {
    if (seen.size >= 800 || length >= TEXT_LIMIT - 3000 || depth > (options.depth ?? 12) || seen.has(node.nodeId)) return;
    seen.add(node.nodeId);
    if (!node.ignored) {
      let ref = "";
      if (node.backendDOMNodeId) {
        ref = ctx.page!.reverse.get(node.backendDOMNodeId) ?? `n${ctx.page!.document}_${++ctx.page!.nextRef}`;
        ctx.page!.reverse.set(node.backendDOMNodeId, ref); ctx.page!.refs.set(ref, node.backendDOMNodeId);
      }
      const properties = node.properties ?? [], protectedValue = properties.some((prop) => prop.name === "protected" && prop.value.value === true);
      let line = `${"  ".repeat(depth)}- ${string(node.role?.value) || "node"} ${JSON.stringify(bounded(string(node.name?.value), 1000))}${ref ? ` [ref=${ref}]` : ""}`;
      for (const prop of properties) if (["checked", "disabled", "expanded", "selected", "readonly", "required"].includes(prop.name)) line += ` [${prop.name}=${String(prop.value.value)}]`;
      if (!protectedValue && node.value?.value !== undefined) line += ` [value=${JSON.stringify(bounded(String(node.value.value), 1000))}]`;
      if (options.boxes && ref && seen.size <= 80) {
        try { const rect = await box(ctx, await element(ctx, ref)); line += ` [box=${[rect.x, rect.y, rect.width, rect.height].map(Math.round).join(",")}]`; } catch { /* detached/non-layout AX nodes have no box */ }
      }
      lines.push(line); length += line.length;
    }
    for (const id of node.childIds ?? []) { const child = map.get(id); if (child) await visit(child, depth + (node.ignored ? 0 : 1)); }
  };
  if (root) await visit(root, 0);
  if (seen.size < nodes.length) lines.push("[Snapshot bounded by requested depth, node count or output size. Use a target or browser_find to narrow it.]");
  return bounded(lines.join("\n"));
}
function inside(root: string, path: string): boolean { const rel = relative(root, path); return rel === "" || (!rel.startsWith(`..${sep}`) && rel !== ".." && !isAbsolute(rel)); }
async function uploads(bot: NativeBrowserBot, paths: string[]): Promise<string[]> {
  const roots = [await realpath(bot.homeFolder)];
  const extra: { projectFolder?: string | null } = bot;
  if (extra.projectFolder) roots.push(await realpath(extra.projectFolder));
  let size = 0;
  return Promise.all(paths.map(async (path) => {
    if (!isAbsolute(path)) throw new Error("Upload paths must be absolute paths inside this bot's folders.");
    const file = await realpath(path), info = await stat(file);
    size += info.size;
    if (!roots.some((root) => inside(root, file)) || !info.isFile() || size > 20 * 1024 * 1024) throw new Error("Uploads must be regular files inside this bot's folders, at most 20 MiB combined.");
    return file;
  }));
}
async function output(ctx: Context, data: string | Buffer, filename: string): Promise<string> {
  const base = await realpath(ctx.bot.homeFolder), directory = join(base, "browser-files");
  await mkdir(directory, { recursive: true });
  if ((await lstat(directory)).isSymbolicLink() || !inside(base, await realpath(directory))) throw new Error("Browser output must remain inside the bot's folder.");
  const path = resolve(directory, filename);
  if (!inside(directory, path) || dirname(path) !== directory || /[<>:"|?*]/.test(basename(path))) throw new Error("Use a plain filename inside this bot's browser-files folder.");
  await writeFile(path, data, { flag: "wx", mode: 0o600 });
  return path;
}
async function result(ctx: Context, value: string, filename?: string): Promise<NativeBrowserToolResult> {
  return filename ? text(`Saved browser output: ${await output(ctx, bounded(value), filename)}`) : text(value);
}
function simpleRegex(value: string): RegExp {
  const match = /^\/(.*)\/([im]*)$/.exec(value), pattern = match?.[1] ?? value, flags = match?.[2] ?? "";
  // One repetition, no grouping/backreferences: matching stays bounded on untrusted page text.
  if (pattern.length > 128 || /[(){}]|\\[1-9]/.test(pattern) || (pattern.match(/(?<!\\)[*+?]/g)?.length ?? 0) > 1) throw new Error("Use a simple regular expression without groups or multiple repetitions, or use plain text.");
  try { return new RegExp(pattern, flags); } catch { throw new Error("Invalid regular expression."); }
}
const modifiers = (values: string[] = []): NonNullable<MouseInputEvent["modifiers"]> => values.map((value) => {
  const names: Record<string, NonNullable<MouseInputEvent["modifiers"]>[number]> = { Alt: "alt", Control: "control", ControlOrMeta: process.platform === "darwin" ? "meta" : "control", Meta: "meta", Shift: "shift" };
  if (!names[value]) throw new Error("Unknown key modifier.");
  return names[value];
});
async function key(ctx: Context, value: string): Promise<void> {
  guard(ctx);
  const parts = value === "+" ? ["+"] : value.endsWith("++") ? [...value.slice(0, -2).split("+"), "+"] : value.split("+"), name = parts.pop()!, mods = modifiers(parts);
  const aliases: Record<string, string> = { ArrowLeft: "Left", ArrowRight: "Right", ArrowUp: "Up", ArrowDown: "Down", Escape: "Escape", " ": "Space" };
  if (!name || parts.some((part) => !["Alt", "Control", "ControlOrMeta", "Meta", "Shift"].includes(part))) throw new Error("Use a key or modifier combination such as Control+A.");
  const keyCode = aliases[name] ?? name;
  if ([...name].length === 1 && name.length > 1 && !mods.length) { await ctx.contents!.insertText(name); guard(ctx); return; }
  try {
    guard(ctx);
    ctx.contents!.sendInputEvent({ type: "keyDown", keyCode, modifiers: mods });
    if (name.length === 1 && !mods.some((value) => value === "control" || value === "meta" || value === "alt")) ctx.contents!.sendInputEvent({ type: "char", keyCode: name, modifiers: mods });
  } finally { ctx.contents!.sendInputEvent({ type: "keyUp", keyCode, modifiers: mods }); }
}
async function click(ctx: Context, args: { target?: string; ref?: string; doubleClick?: boolean; button?: "left" | "right" | "middle"; modifiers?: string[] }): Promise<void> {
  const point = await actionable(ctx, target(args)), button = args.button ?? "left", mods = modifiers(args.modifiers);
  guard(ctx);
    ctx.contents!.sendInputEvent({ type: "mouseMove", x: Math.round(point.x), y: Math.round(point.y), modifiers: mods });
    for (let count = 1; count <= (args.doubleClick ? 2 : 1); count++) {
      try { ctx.contents!.sendInputEvent({ type: "mouseDown", x: Math.round(point.x), y: Math.round(point.y), button, clickCount: count, modifiers: mods }); }
      finally { ctx.contents!.sendInputEvent({ type: "mouseUp", x: Math.round(point.x), y: Math.round(point.y), button, clickCount: count, modifiers: mods }); }
    }
    await delay(ctx, 40);
}
async function fill(ctx: Context, value: string, input: string, slowly = false): Promise<void> {
  const { objectId } = await actionable(ctx, value, true);
  await call(ctx, objectId, `function(clear) {
    this.focus(); if (!clear) return;
    if (this instanceof HTMLInputElement || this instanceof HTMLTextAreaElement) { this.select(); }
    else { const range = document.createRange(); range.selectNodeContents(this); const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range); }
  }`, [!slowly]);
  if (slowly) { for (const char of input) { await key(ctx, char); } }
  else { guard(ctx); if (input) await ctx.contents!.insertText(input); else await key(ctx, "Backspace"); guard(ctx); }
}
async function select(ctx: Context, value: string, values: string[], byLabel = false): Promise<void> {
  const { objectId } = await actionable(ctx, value);
  await call(ctx, objectId, `function(values, byLabel) {
    if (!(this instanceof HTMLSelectElement)) throw new Error('Target is not a native select element. Use click/type for a custom combobox.');
    const chosen = Array.from(this.options).filter(option => values.includes(byLabel ? option.label : option.value));
    if (chosen.length !== values.length || (!this.multiple && chosen.length !== 1) || chosen.some(option => option.disabled)) throw new Error('Requested options are missing, disabled or incompatible.');
    for (const option of this.options) option.selected = chosen.includes(option);
    this.dispatchEvent(new Event('input', {bubbles:true})); this.dispatchEvent(new Event('change', {bubbles:true}));
  }`, [values, byLabel]);
}
async function afterNavigation(ctx: Context): Promise<NativeBrowserToolResult> {
  return ctx.engine.withAutomation(ctx.bot, {}, async (contents) => {
    ctx.contents = contents; active.set(contents, ctx);
    try {
      await prepare(ctx);
      await delay(ctx, 60);
      const until = Math.min(ctx.deadline - 1000, Date.now() + ACTION_MS);
      while (contents.isLoadingMainFrame() && Date.now() < until && !ctx.engine.state(ctx.bot.id).dialog) await delay(ctx, 50);
      return text(`${contents.isLoadingMainFrame() ? "Navigation is still loading.\n" : ""}${await snapshot(ctx)}`);
    } finally { active.delete(contents); }
  });
}
async function checkbox(ctx: Context, value: string, checked: boolean): Promise<void> {
  const item = await actionable(ctx, value);
  const current = await call(ctx, item.objectId, "function() { if (!(this instanceof HTMLInputElement) || !['checkbox','radio'].includes(this.type)) throw new Error('Target is not a checkbox or radio input.'); return this.checked; }");
  if (current !== checked) await click(ctx, { target: value });
}
async function capture(ctx: Context, args: { target?: string; ref?: string; fullPage?: boolean; type?: "png" | "jpeg" | "webp"; filename?: string; scale?: "css" | "device" }): Promise<NativeBrowserToolResult> {
  const type = args.type ?? "png";
  if (args.fullPage && (args.target || args.ref)) throw new Error("Choose fullPage or an element target, not both.");
  let clip: { x: number; y: number; width: number; height: number; scale: number } | undefined;
  const metrics = await cdp(ctx.contents!, "Page.getLayoutMetrics"), viewport = record(metrics.cssVisualViewport), content = record(metrics.cssContentSize);
  const viewportScale = args.scale === "device" ? 1 : 1 / Number(await evaluate(ctx, "window.devicePixelRatio || 1"));
  if (args.target || args.ref) {
    const item = await actionable(ctx, target(args));
    clip = { ...await box(ctx, item.objectId), scale: viewportScale };
    clip.x += Number(viewport.pageX ?? 0); clip.y += Number(viewport.pageY ?? 0);
  } else if (args.fullPage) clip = { x: 0, y: 0, width: Number(content.width), height: Number(content.height), scale: viewportScale };
  else clip = { x: Number(viewport.pageX ?? 0), y: Number(viewport.pageY ?? 0), width: Number(viewport.clientWidth), height: Number(viewport.clientHeight), scale: viewportScale };
  if (Object.values(clip).some((value) => !Number.isFinite(value)) || clip.width <= 0 || clip.height <= 0 || clip.width * clip.height > 16_000_000 || clip.width > 16000 || clip.height > 16000 || clip.scale <= 0 || clip.scale > 4) throw new Error("The screenshot exceeds the 16-megapixel limit. Use a smaller viewport or an element target.");
  const data = string((await cdp(ctx.contents!, "Page.captureScreenshot", { format: type, ...(type === "png" ? {} : { quality: 75 }), clip, captureBeyondViewport: !!args.fullPage || !!args.target || !!args.ref })).data);
  if (!data || data.length > Math.ceil(IMAGE_LIMIT / 3) * 4 || !/^[A-Za-z0-9+/]*={0,2}$/.test(data)) throw new Error("The screenshot exceeds the 2 MiB image limit. Use JPEG or a smaller target.");
  const bytes = Buffer.from(data, "base64");
  if (bytes.length > IMAGE_LIMIT) throw new Error("The screenshot exceeds the 2 MiB image limit.");
  const saved = args.filename ? `Saved screenshot: ${await output(ctx, bytes, args.filename)}` : "Screenshot of the shared native browser tab.";
  return { content: [{ type: "text", text: saved }, { type: "image", data, mimeType: `image/${type}` }] };
}
const optionalTarget = { element: z.string().max(1000).optional(), target: z.string().min(1).max(2000).optional(), ref: z.string().min(1).max(2000).optional() };
const requireTarget = (args: { target?: string; ref?: string }): boolean => !!(args.target ?? args.ref) && (!args.target || !args.ref || args.target === args.ref);
const filename = z.string().min(1).max(255).optional();
const empty = z.object({}).strict();
type Definition = { description: string; input: z.ZodObject; page: boolean; run(ctx: Context, args: unknown): Promise<NativeBrowserToolResult> };
function define<S extends z.ZodObject>(description: string, input: S, page: boolean, run: (ctx: Context, args: z.output<S>) => Promise<NativeBrowserToolResult>): Definition {
  return { description, input, page, run: (ctx, args) => run(ctx, input.parse(args)) };
}
async function stateTarget(ctx: Context): Promise<{ generation: string; pageId: string }> {
  const state = await ctx.engine.open(ctx.bot);
  guard(ctx);
  if (!state.activePageId) throw new Error("No browser tab is open.");
  return { generation: state.generation, pageId: state.activePageId };
}
function stateText(ctx: Context): NativeBrowserToolResult {
  const state = ctx.engine.state(ctx.bot.id);
  return text(state.tabs.map((tab, index) => `- ${index}: ${tab.id === state.activePageId ? "(current) " : ""}${JSON.stringify(tab.title)} (${tab.url})`).join("\n") || "No open tabs.");
}
const act = (ctx: Context, command: NativeBrowserUiCommand) => ctx.engine.act(ctx.bot, command, "bot", () => guard(ctx));

// Runtime imports are Node + Zod only. The daemon reuses these exact validators/metadata without
// importing Electron at runtime; tool execution stays in the desktop process under withAutomation.
export const nativeBrowserTools: Record<string, Definition> = {
  browser_snapshot: define("Bounded accessibility snapshot with document-scoped refs. Use these refs for actions.", z.object({ ...optionalTarget, filename, depth: z.number().int().min(0).max(30).optional(), boxes: z.boolean().optional() }).strict(), true, async (ctx, args) => result(ctx, await snapshot(ctx, args), args.filename)),
  browser_click: define("Click a visible, enabled, stable, unobscured element from a snapshot ref or unique CSS target.", z.object({ ...optionalTarget, doubleClick: z.boolean().optional(), button: z.enum(["left", "right", "middle"]).optional(), modifiers: z.array(z.enum(["Alt", "Control", "ControlOrMeta", "Meta", "Shift"])).max(5).optional() }).strict(), true, async (ctx, args) => { await click(ctx, args); return text(await snapshot(ctx)); }),
  browser_hover: define("Hover over an actionable element.", z.object(optionalTarget).strict(), true, async (ctx, args) => { const point = await actionable(ctx, target(args)); ctx.contents!.sendInputEvent({ type: "mouseMove", x: Math.round(point.x), y: Math.round(point.y) }); return text(await snapshot(ctx)); }),
  browser_type: define("Fill an editable element, or type key by key with slowly. Optionally press Enter.", z.object({ ...optionalTarget, text: z.string().max(100_000), slowly: z.boolean().optional(), submit: z.boolean().optional() }).strict(), true, async (ctx, args) => { if (args.slowly && args.text.length > 2000) throw new Error("Slow typing is limited to 2000 characters."); await fill(ctx, target(args), args.text, args.slowly); if (args.submit) await key(ctx, "Enter"); return text(await snapshot(ctx)); }),
  browser_select_option: define("Select existing options by value in a native select element.", z.object({ ...optionalTarget, values: z.array(z.string().max(2000)).min(1).max(100) }).strict(), true, async (ctx, args) => { await select(ctx, target(args), args.values); return text(await snapshot(ctx)); }),
  browser_fill_form: define("Fill native textboxes, checkboxes, radios, select boxes and sliders from snapshot refs.", z.object({ fields: z.array(z.object({ ...optionalTarget, name: z.string().max(1000), type: z.enum(["textbox", "checkbox", "radio", "combobox", "slider"]), value: z.string().max(100_000) }).strict().refine(requireTarget, "An exact target or ref is required.")).min(1).max(30) }).strict(), true, async (ctx, args) => {
    for (const field of args.fields) {
      const ref = target(field);
      if (field.type === "textbox") await fill(ctx, ref, field.value);
      else if (field.type === "combobox") await select(ctx, ref, [field.value], true);
      else if (field.type === "checkbox" || field.type === "radio") { if (!["true", "false"].includes(field.value)) throw new Error("Checkbox and radio values must be true or false."); await checkbox(ctx, ref, field.value === "true"); }
      else {
        const item = await actionable(ctx, ref);
        await call(ctx, item.objectId, "function(value) { if (!(this instanceof HTMLInputElement) || this.type !== 'range' || !Number.isFinite(Number(value))) throw new Error('A slider requires a range input and numeric value.'); const min = Number(this.min || 0), max = Number(this.max || 100); if (Number(value) < min || Number(value) > max) throw new Error('Slider value is outside its range.'); this.value = value; this.dispatchEvent(new Event('input',{bubbles:true})); this.dispatchEvent(new Event('change',{bubbles:true})); }", [field.value]);
      }
    }
    return text(await snapshot(ctx));
  }),
  browser_drag: define("Drag between visible, stable elements using native mouse input.", z.object({ startElement: z.string().max(1000).optional(), startTarget: z.string().max(2000).optional(), startRef: z.string().max(2000).optional(), endElement: z.string().max(1000).optional(), endTarget: z.string().max(2000).optional(), endRef: z.string().max(2000).optional() }).strict(), true, async (ctx, args) => {
    const from = await actionable(ctx, target({ target: args.startTarget, ref: args.startRef })), to = await actionable(ctx, target({ target: args.endTarget, ref: args.endRef }));
    guard(ctx); const start = await box(ctx, from.objectId), finish = await box(ctx, to.objectId);
    let x = Math.round(start.x + start.width / 2), y = Math.round(start.y + start.height / 2);
    try {
      guard(ctx); ctx.contents!.sendInputEvent({ type: "mouseMove", x, y }); ctx.contents!.sendInputEvent({ type: "mouseDown", x, y, button: "left", clickCount: 1 });
      for (let step = 1; step <= 10; step++) { await delay(ctx, 15); x = Math.round(start.x + start.width / 2 + (finish.x + finish.width / 2 - start.x - start.width / 2) * step / 10); y = Math.round(start.y + start.height / 2 + (finish.y + finish.height / 2 - start.y - start.height / 2) * step / 10); ctx.contents!.sendInputEvent({ type: "mouseMove", x, y, button: "left", modifiers: ["leftbuttondown"] }); }
    } finally { ctx.contents!.sendInputEvent({ type: "mouseUp", x, y, button: "left", clickCount: 1 }); }
    return text(await snapshot(ctx));
  }),
  browser_drop: define("Drop authorized bot-folder files (20 MiB total) or bounded MIME text data on a visible element.", z.object({ ...optionalTarget, paths: z.array(z.string().max(4096)).min(1).max(20).optional(), data: z.record(z.string().min(1).max(100), z.string().max(100_000)).optional() }).strict(), true, async (ctx, args) => {
    if (!args.paths?.length && !Object.keys(args.data ?? {}).length) throw new Error("Supply paths or MIME data.");
    if (Object.keys(args.data ?? {}).length > 20) throw new Error("At most 20 MIME data items are supported.");
    const files = args.paths ? await uploads(ctx.bot, args.paths) : [], point = await actionable(ctx, target(args));
    const data = { items: Object.entries(args.data ?? {}).map(([mimeType, data]) => ({ mimeType, data })), files, dragOperationsMask: 1 };
    for (const type of ["dragEnter", "dragOver", "drop"]) await cdp(ctx.contents!, "Input.dispatchDragEvent", { type, x: point.x, y: point.y, data });
    return text(await snapshot(ctx));
  }),
  browser_emulate_media: define("Set page CSS media preferences; null resets a previously set override.", z.object({ colorScheme: z.enum(["light", "dark", "no-preference"]).nullable().optional(), reducedMotion: z.enum(["reduce", "no-preference"]).nullable().optional(), forcedColors: z.enum(["active", "none"]).nullable().optional(), contrast: z.enum(["more", "no-preference"]).nullable().optional(), media: z.enum(["screen", "print"]).nullable().optional() }).strict(), true, async (ctx, args) => {
    const features = { colorScheme: "prefers-color-scheme", reducedMotion: "prefers-reduced-motion", forcedColors: "forced-colors", contrast: "prefers-contrast" };
    for (const [key, name] of Object.entries(features)) { const value = record(args)[key]; if (value === null) ctx.page!.media.delete(name); else if (typeof value === "string") ctx.page!.media.set(name, value); }
    if (args.media !== undefined) ctx.page!.mediaType = args.media ?? undefined;
    await cdp(ctx.contents!, "Emulation.setEmulatedMedia", { media: ctx.page!.mediaType ?? "", features: [...ctx.page!.media].map(([name, value]) => ({ name, value })) });
    return text(await snapshot(ctx));
  }),
  browser_take_screenshot: define("Capture the shared tab, full page or element (16 megapixels / 2 MiB maximum). Optional plain filename saves in the bot's browser-files folder.", z.object({ ...optionalTarget, type: z.enum(["png", "jpeg", "webp"]).optional(), filename, fullPage: z.boolean().optional(), scale: z.enum(["css", "device"]).optional() }).strict(), true, capture),
  browser_resize: define("Resize an unmounted bot viewport. A visible native browser follows the actual Tjommie panel and cannot be resized by this tool.", z.object({ width: z.number().int().min(240).max(2560), height: z.number().int().min(180).max(1600) }).strict(), false, async (ctx, args) => { await ctx.engine.open(ctx.bot); await ctx.engine.resize(ctx.bot, args.width, args.height, () => guard(ctx)); return stateText(ctx); }),
  browser_press_key: define("Press a key or modifier combination such as Enter, ArrowLeft or Control+A.", z.object({ key: z.string().min(1).max(100) }).strict(), true, async (ctx, args) => { await key(ctx, args.key); await pause(20); return text(await snapshot(ctx)); }),
  browser_evaluate: define("Evaluate page-context JavaScript, optionally with an element. No Node or desktop APIs are exposed.", z.object({ ...optionalTarget, function: z.string().min(1).max(50_000), filename }).strict(), true, async (ctx, args) => {
    const serialize = `const serialized = JSON.stringify(value ?? null, null, 2); return serialized.length > ${TEXT_LIMIT} ? serialized.slice(0, ${TEXT_LIMIT}) + '\\n[truncated]' : serialized;`;
    const value = args.target || args.ref ? await call(ctx, await element(ctx, target(args)), `async function() { const fn = (${args.function}); const value = typeof fn === 'function' ? await fn(this) : fn; ${serialize} }`) : await evaluate(ctx, `(async () => { const fn = (${args.function}); const value = typeof fn === 'function' ? await fn() : fn; ${serialize} })()`);
    return result(ctx, typeof value === "string" ? value : JSON.stringify(value ?? null), args.filename);
  }),
  browser_navigate: define("Navigate the shared native tab to an HTTP/HTTPS address or about:blank.", z.object({ url: z.string().min(1).max(8192) }).strict(), false, async (ctx, args) => { await act(ctx, { kind: "navigate", ...await stateTarget(ctx), url: args.url }); return afterNavigation(ctx); }),
  browser_navigate_back: define("Go back in the active tab's history.", empty, false, async (ctx) => { await act(ctx, { kind: "history", ...await stateTarget(ctx), action: "back" }); return afterNavigation(ctx); }),
  browser_navigate_forward: define("Go forward in the active tab's history.", empty, false, async (ctx) => { await act(ctx, { kind: "history", ...await stateTarget(ctx), action: "forward" }); return afterNavigation(ctx); }),
  browser_reload: define("Reload the active tab.", empty, false, async (ctx) => { await act(ctx, { kind: "history", ...await stateTarget(ctx), action: "reload" }); return afterNavigation(ctx); }),
  browser_close: define("Close the active page while preserving this bot's persistent browser session.", empty, false, async (ctx) => { await act(ctx, { kind: "tab", ...await stateTarget(ctx), action: "close" }); return stateText(ctx); }),
  browser_tabs: define("List, create, close or select tabs in the same native browser the person sees.", z.object({ action: z.enum(["list", "new", "close", "select"]), index: z.number().int().min(0).max(99).optional(), url: z.string().max(8192).optional() }).strict(), false, async (ctx, args) => {
    let state = await ctx.engine.open(ctx.bot);
    guard(ctx);
    if (args.action !== "list") {
      const selected = args.index === undefined ? state.activePageId ?? undefined : state.tabs[args.index]?.id;
      if ((args.index !== undefined && !selected) || (args.action === "select" && args.index === undefined)) throw new Error("Choose an existing tab index.");
      state = await act(ctx, { kind: "tab", generation: state.generation, action: args.action, pageId: selected });
      if (args.action === "new" && args.url && state.activePageId) { await act(ctx, { kind: "navigate", generation: state.generation, pageId: state.activePageId, url: args.url }); return afterNavigation(ctx); }
    }
    return stateText(ctx);
  }),
  browser_handle_dialog: define("Accept or dismiss the current page dialog, including a background tab; can unblock an in-flight action.", z.object({ accept: z.boolean(), promptText: z.string().max(10_000).optional() }).strict(), false, async (ctx, args) => {
    const state = await ctx.engine.open(ctx.bot); guard(ctx);
    if (!state.dialog) throw new Error("No page dialog is open.");
    await act(ctx, { kind: "dialog", generation: state.generation, pageId: state.dialog.pageId, ...args }); return stateText(ctx);
  }),
  browser_console_messages: define("Read up to 200 console messages observed during bot control; human sign-in output is not recorded.", z.object({ level: z.enum(["error", "warning", "info", "debug"]).default("info"), all: z.boolean().optional(), filename }).strict(), true, async (ctx, args) => {
    const rank: Record<string, number> = { error: 0, assert: 0, warning: 1, warn: 1, info: 2, log: 2, debug: 3 };
    return result(ctx, ctx.page!.console.filter((entry) => (args.all || entry.document === ctx.page!.document) && (rank[entry.level] ?? 2) <= rank[args.level]).map((entry) => `[${entry.level}] ${entry.text}`).join("\n") || "No matching observed console messages.", args.filename);
  }),
  browser_network_requests: define("List up to 200 observed bot-control network requests. Credential headers are redacted; request bodies are not retained.", z.object({ static: z.boolean().default(false), includeStatic: z.boolean().optional(), filter: z.string().max(128).optional(), filename }).strict(), true, async (ctx, args) => {
    const regex = args.filter ? simpleRegex(args.filter) : null;
    return result(ctx, [...ctx.page!.requests.values()].filter((entry) => ((args.static || args.includeStatic) || ["Fetch", "XHR", "Document"].includes(entry.type) || (entry.status ?? 0) >= 400) && (!regex || regex.test(entry.url))).map((entry) => `${entry.index}. ${entry.method} ${entry.url} => ${entry.status ?? "pending"}`).join("\n") || "No matching observed requests.", args.filename);
  }),
  browser_network_request: define("Read bounded observed request metadata or an explicitly requested response body. Credential headers are redacted; request bodies are not retained.", z.object({ index: z.number().int().min(1), part: z.enum(["request-headers", "request-body", "response-headers", "response-body"]).optional(), filename }).strict(), true, async (ctx, args) => {
    const entry = [...ctx.page!.requests.values()].find((request) => request.index === args.index);
    if (!entry) throw new Error("That request is no longer in the bounded network log.");
    let value: unknown = { url: entry.url, method: entry.method, status: entry.status, requestHeaders: entry.requestHeaders, responseHeaders: entry.responseHeaders };
    if (args.part === "request-body") value = "Request bodies are not retained.";
    else if (args.part === "request-headers") value = entry.requestHeaders;
    else if (args.part === "response-headers") value = entry.responseHeaders ?? {};
    else if (args.part === "response-body") { const body = await cdp(ctx.contents!, "Network.getResponseBody", { requestId: entry.id }); value = body.base64Encoded ? { base64: bounded(string(body.body)), truncated: string(body.body).length > TEXT_LIMIT } : bounded(string(body.body)); }
    return result(ctx, typeof value === "string" ? value : JSON.stringify(value, null, 2), args.filename);
  }),
  browser_file_upload: define("Set a pending bot-opened file chooser. Files must be in this bot's authorized folders, at most 20 MiB total. Omit paths to cancel.", z.object({ paths: z.array(z.string().min(1).max(4096)).max(20).optional() }).strict(), true, async (ctx, args) => {
    if (!ctx.page!.fileInput) throw new Error("No bot-opened file chooser is pending. Click the file input first.");
    if (args.paths) await cdp(ctx.contents!, "DOM.setFileInputFiles", { backendNodeId: ctx.page!.fileInput, files: await uploads(ctx.bot, args.paths) });
    ctx.page!.fileInput = undefined; return text(await snapshot(ctx));
  }),
  browser_wait_for: define("Wait up to 20 seconds, or up to 10 seconds for visible page text to appear/disappear; the total tool deadline is 22 seconds.", z.object({ time: z.number().min(0).max(20).optional(), text: z.string().min(1).max(2000).optional(), textGone: z.string().min(1).max(2000).optional() }).strict(), true, async (ctx, args) => {
    if (args.time === undefined && !args.text && !args.textGone) throw new Error("Supply time, text or textGone.");
    if (args.time) await delay(ctx, args.time * 1000);
    const deadline = Date.now() + ACTION_MS;
    for (;;) {
      const ok = await evaluate(ctx, `(() => { const body = document.body?.innerText ?? ''; return ${args.text ? `body.includes(${JSON.stringify(args.text)})` : "true"} && ${args.textGone ? `!body.includes(${JSON.stringify(args.textGone)})` : "true"}; })()`);
      if (ok === true) break;
      if (Date.now() >= deadline) throw new Error("The requested text condition did not become true.");
      await delay(ctx, 100);
    }
    return text(await snapshot(ctx));
  }),
  browser_find: define("Search the bounded accessibility snapshot using plain text or a simple regular expression (no groups or multiple repetitions).", z.object({ text: z.string().min(1).max(2000).optional(), regex: z.string().min(1).max(128).optional() }).strict(), true, async (ctx, args) => {
    if (!!args.text === !!args.regex) throw new Error("Supply text or regex, not both.");
    const lines = (await snapshot(ctx)).split("\n"), pattern = args.regex ? simpleRegex(args.regex) : null;
    const hits = new Set<number>();
    lines.forEach((line, index) => { if (pattern ? pattern.test(line) : line.toLowerCase().includes(args.text!.toLowerCase())) for (let i = Math.max(0, index - 1); i <= Math.min(lines.length - 1, index + 1); i++) hits.add(i); });
    return text([...hits].slice(0, 100).map((index) => lines[index]).join("\n") || "No matches in the bounded snapshot.");
  }),
};

export function validateNativeBrowserTool(name: string, args: unknown): NativeBrowserToolCommand {
  const definition = Object.hasOwn(nativeBrowserTools, name) ? nativeBrowserTools[name] : undefined;
  if (!definition) throw new Error(`Unsupported native browser tool: ${bounded(name, 100)}.`);
  const parsed = definition.input.safeParse(args);
  if (!parsed.success) throw new Error(`Invalid arguments for ${name}: ${parsed.error.issues.map((issue) => `${issue.path.join(".") || "arguments"}: ${issue.message}`).join("; ")}`);
  const value = record(parsed.data);
  if (["browser_click", "browser_hover", "browser_type", "browser_select_option", "browser_drop"].includes(name)) target({ target: string(value.target) || undefined, ref: string(value.ref) || undefined });
  if (name === "browser_drag") { target({ target: string(value.startTarget) || undefined, ref: string(value.startRef) || undefined }); target({ target: string(value.endTarget) || undefined, ref: string(value.endRef) || undefined }); }
  if (name === "browser_find" && !!value.text === !!value.regex) throw new Error("Supply text or regex, not both.");
  if (name === "browser_wait_for" && value.time === undefined && !value.text && !value.textGone) throw new Error("Supply time, text or textGone.");
  if (name === "browser_tabs" && value.url && value.action !== "new") throw new Error("A URL is supported only when creating a new tab.");
  return { kind: "tool", name, args: value };
}

export async function nativeBrowserAutomation(engine: NativeBrowserEngine, bot: NativeBrowserBot, raw: NativeBrowserToolCommand): Promise<NativeBrowserToolResult> {
  const ctx: Context = { engine, bot, deadline: Date.now() + 22_000 };
  const contexts = activeBots.get(bot.id) ?? new Set<Context>();
  contexts.add(ctx); activeBots.set(bot.id, contexts);
  try {
    const command = validateNativeBrowserTool(raw.name, raw.args), definition = nativeBrowserTools[command.name];
    if (!definition.page) return await definition.run(ctx, command.args);
    return await engine.withAutomation(bot, {}, async (contents) => {
      ctx.contents = contents;
      active.set(contents, ctx);
      try { await prepare(ctx); return await definition.run(ctx, command.args); }
      finally { active.delete(contents); void contents.debugger.sendCommand("Runtime.releaseObjectGroup", { objectGroup: "tjommie-tools" }).catch(() => {}); }
    });
  } catch (error) { return { ...text(error instanceof Error ? error.message : "The native browser tool failed."), isError: true }; }
  finally { contexts.delete(ctx); if (!contexts.size) activeBots.delete(bot.id); }
}
