import { z } from "zod";
import { BrowserInput, StreamedBrowserFrame } from "./browser.ts";

const Page = { generation: z.string().min(1).max(100), pageId: z.string().min(1).max(100) };
export const NativeBrowserBounds = z.object({
  x: z.number().finite().min(0).max(32768), y: z.number().finite().min(0).max(32768),
  width: z.number().finite().min(0).max(32768), height: z.number().finite().min(0).max(32768),
}).strict();
export type NativeBrowserBounds = z.infer<typeof NativeBrowserBounds>;
export const NativeBrowserMount = z.object({ visible: z.boolean(), bounds: NativeBrowserBounds, occluded: z.boolean().optional() }).strict();
export type NativeBrowserMount = z.infer<typeof NativeBrowserMount>;

// Renderer IPC accepts ONLY this subset. Tool commands belong to the private desktop-host bridge.
export const NativeBrowserUiCommand = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("open") }).strict(),
  z.object({ kind: z.literal("state") }).strict(),
  z.object({ kind: z.literal("control"), control: z.enum(["human", "bot"]) }).strict(),
  z.object({ kind: z.literal("navigate"), ...Page, url: z.string().min(1).max(8192) }).strict(),
  z.object({ kind: z.literal("history"), ...Page, action: z.enum(["back", "forward", "reload"]) }).strict(),
  z.object({ kind: z.literal("tab"), generation: Page.generation, action: z.enum(["new", "select", "close"]), pageId: Page.pageId.optional() }).strict(),
  z.object({ kind: z.literal("dialog"), ...Page, accept: z.boolean(), promptText: z.string().max(10000).optional() }).strict(),
]);
export type NativeBrowserUiCommand = z.infer<typeof NativeBrowserUiCommand>;
export const NativeBrowserToolCommand = z.object({
  kind: z.literal("tool"), name: z.string().regex(/^browser_[a-z_]+$/).max(100),
  args: z.record(z.string().max(100), z.unknown()).refine((value) => {
    try { return JSON.stringify(value).length <= 1_000_000; } catch { return false; }
  }, "Browser tool arguments exceed the size limit"),
}).strict();
export type NativeBrowserToolCommand = z.infer<typeof NativeBrowserToolCommand>;
const Controlled = { controlEpoch: z.number().int().nonnegative(), ...Page };
// Only the authenticated daemon adds controllerId. This command is excluded from renderer IPC.
export const NativeBrowserRemoteAction = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("state") }).strict(),
  z.object({ kind: z.literal("frame"), thumbnail: z.boolean().optional() }).strict(),
  z.object({ kind: z.literal("disconnect") }).strict(),
  z.object({ kind: z.literal("stream"), on: z.boolean(), maxFps: z.number().int().min(1).max(60).optional() }).strict(),
  z.object({ kind: z.literal("control"), control: z.enum(["human", "bot"]), generation: Page.generation.optional(), controlEpoch: Controlled.controlEpoch.optional(), releaseOnly: z.boolean().optional() }).strict(),
  z.object({ kind: z.literal("navigate"), ...Controlled, url: z.string().min(1).max(8192) }).strict(),
  z.object({ kind: z.literal("history"), ...Controlled, action: z.enum(["back", "forward", "reload"]) }).strict(),
  z.object({ kind: z.literal("tab"), generation: Page.generation, controlEpoch: Controlled.controlEpoch, action: z.enum(["new", "select", "close"]), pageId: Page.pageId.optional() }).strict(),
  z.object({ kind: z.literal("dialog"), ...Controlled, accept: z.boolean(), promptText: z.string().max(10000).optional() }).strict(),
  z.object({ kind: z.literal("resize"), ...Controlled, width: z.number().int().min(240).max(2560), height: z.number().int().min(180).max(1600) }).strict(),
  z.object({ kind: z.literal("input"), ...Controlled, viewportRevision: z.number().int().nonnegative(), input: BrowserInput }).strict(),
]);
export type NativeBrowserRemoteAction = z.infer<typeof NativeBrowserRemoteAction>;
export const NativeBrowserRemoteCommand = z.object({ kind: z.literal("remote"), controllerId: z.string().min(1).max(256), action: NativeBrowserRemoteAction }).strict();
export type NativeBrowserRemoteCommand = z.infer<typeof NativeBrowserRemoteCommand>;
// What the desktop host posts to /browser-host on its own, outside any job: live frames, and the end of a stream.
export const NativeBrowserStreamMessage = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("stream_frame"), botId: z.string().min(1).max(256), frame: StreamedBrowserFrame }).strict(),
  z.object({ kind: z.literal("stream_end"), botId: z.string().min(1).max(256), reason: z.enum(["private", "closed", "failed"]) }).strict(),
]);
export type NativeBrowserStreamMessage = z.infer<typeof NativeBrowserStreamMessage>;
export const NativeBrowserCommand = z.union([NativeBrowserUiCommand, NativeBrowserToolCommand,
  NativeBrowserRemoteCommand,
  z.object({ kind: z.literal("close") }).strict(), z.object({ kind: z.literal("delete_profile") }).strict(),
  z.object({ kind: z.literal("cancel_tools") }).strict()]);
export type NativeBrowserCommand = z.infer<typeof NativeBrowserCommand>;
