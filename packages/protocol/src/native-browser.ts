import { z } from "zod";

const Page = { generation: z.string().min(1).max(100), pageId: z.string().min(1).max(100) };
export const NativeBrowserBounds = z.object({
  x: z.number().finite().min(0).max(32768), y: z.number().finite().min(0).max(32768),
  width: z.number().finite().min(0).max(32768), height: z.number().finite().min(0).max(32768),
}).strict();
export type NativeBrowserBounds = z.infer<typeof NativeBrowserBounds>;
export const NativeBrowserMount = z.object({ visible: z.boolean(), bounds: NativeBrowserBounds }).strict();
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
export const NativeBrowserCommand = z.union([NativeBrowserUiCommand, NativeBrowserToolCommand,
  z.object({ kind: z.literal("close") }).strict(), z.object({ kind: z.literal("delete_profile") }).strict(),
  z.object({ kind: z.literal("cancel_tools") }).strict()]);
export type NativeBrowserCommand = z.infer<typeof NativeBrowserCommand>;
