import { z } from "zod";

export const BrowserControl = z.enum(["human", "bot"]);
export const BrowserState = z.object({
  botId: z.string(),
  generation: z.string(),
  control: BrowserControl,
  revision: z.number().int().nonnegative().optional(),
  viewportRevision: z.number().int().nonnegative().optional(),
  busy: z.boolean().optional(),
  activePageId: z.string().nullable(),
  tabs: z.array(z.object({ id: z.string(), url: z.string(), title: z.string() })),
  dialog: z.object({ pageId: z.string(), type: z.string(), message: z.string(), defaultValue: z.string() }).nullable(),
});
export type BrowserState = z.infer<typeof BrowserState>;

const Coordinate = z.number().finite().min(0).max(4096);
const Button = z.enum(["left", "middle", "right"]);
export const BrowserInput = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("mouse"), action: z.enum(["move", "down", "up"]), x: Coordinate, y: Coordinate, button: Button.optional() }),
  z.object({ kind: z.literal("wheel"), x: Coordinate, y: Coordinate, deltaX: z.number().finite().min(-10000).max(10000), deltaY: z.number().finite().min(-10000).max(10000) }),
  z.object({ kind: z.literal("key"), action: z.enum(["down", "up"]), key: z.string().min(1).max(80) }),
  z.object({ kind: z.literal("text"), text: z.string().max(100_000) }),
]);
export type BrowserInput = z.infer<typeof BrowserInput>;
export const BrowserFrame = z.object({
  data: z.string(), mime: z.literal("image/jpeg"), width: z.number().int(), height: z.number().int(), sequence: z.number().int(),
  // Encoded pixels can be downsampled. Input always uses the separately reported CSS viewport.
  viewportWidth: z.number().int().positive().optional(), viewportHeight: z.number().int().positive().optional(),
  pageId: z.string().optional(), viewportRevision: z.number().int().nonnegative().optional(), streamId: z.string().optional(),
});
export type BrowserFrame = z.infer<typeof BrowserFrame>;

const Bot = { botId: z.string().min(1) };
const Session = { ...Bot, generation: z.string().min(1) };
const Page = { ...Session, pageId: z.string().min(1) };
export const browserMethods = {
  "browser.open": { params: z.object(Bot), result: BrowserState },
  "browser.frame": { params: z.object({ ...Bot, after: z.object({ streamId: z.string(), sequence: z.number().int().nonnegative() }).optional() }), result: z.object({ state: BrowserState, frame: BrowserFrame.nullable(), unchanged: z.boolean().optional() }) },
  "browser.control": { params: z.object({ ...Bot, control: BrowserControl }), result: BrowserState },
  "browser.navigate": { params: z.object({ ...Page, url: z.string().max(8192) }), result: BrowserState },
  "browser.history": { params: z.object({ ...Page, action: z.enum(["back", "forward", "reload"]) }), result: BrowserState },
  "browser.tab": { params: z.object({ ...Session, action: z.enum(["new", "select", "close"]), pageId: z.string().optional() }), result: BrowserState },
  "browser.resize": { params: z.object({ ...Page, width: z.number().int().min(240).max(2560), height: z.number().int().min(180).max(1600) }), result: BrowserState },
  "browser.input": { params: z.object({ ...Page, input: BrowserInput, viewportRevision: z.number().int().nonnegative().optional() }), result: z.object({}) },
  "browser.dialog": { params: z.object({ ...Page, accept: z.boolean(), promptText: z.string().max(10_000).optional() }), result: BrowserState },
} as const;
