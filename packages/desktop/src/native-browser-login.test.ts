import { Window } from "happy-dom";
import { afterEach, expect, it, vi } from "vitest";
import { prepareNativeLogin, fillNativeLogin, clearNativeLogin } from "./native-browser-login.ts";

const window = new Window({ url: "https://login.invalid/signin" }), document = window.document;

afterEach(() => { document.body.innerHTML = ""; vi.restoreAllMocks(); });
function fixture(html = '<form method="post" action="https://login.invalid/signin"><input autocomplete="username"><input type="password" autocomplete="current-password"><button>Sign in</button></form>') {
  document.body.innerHTML = html;
  vi.spyOn(window.HTMLElement.prototype, "getBoundingClientRect").mockReturnValue(new window.DOMRect(0, 0, 100, 30));
  const location = { origin: "https://login.invalid", href: "https://login.invalid/signin" };
  let fields: unknown, framed = false;
  const execute = (code: string): unknown => new Function("document", "location", "HTMLInputElement", "getComputedStyle", "Event", `return (${code});`)(document, location, window.HTMLInputElement, () => ({ display: "block", visibility: "visible" }), window.Event);
  const api = { sendCommand: vi.fn(async (method: string, params?: Record<string, unknown>): Promise<unknown> => {
    if (method === "Page.getFrameTree") return { frameTree: { frame: { id: "main", url: location.href }, ...(framed ? { childFrames: [{}] } : {}) } };
    if (method === "Page.createIsolatedWorld") return { executionContextId: 9 };
    if (method === "Runtime.evaluate") { fields = execute(String(params?.expression)); return { result: fields ? { objectId: "private-fields" } : { subtype: "null" } }; }
    if (method === "Runtime.callFunctionOn") {
      const fn = execute(String(params?.functionDeclaration)) as (...values: unknown[]) => unknown;
      const values = (params?.arguments as { value: unknown }[] | undefined)?.map((value) => value.value) ?? [];
      return { result: { value: fn.apply(fields, values) } };
    }
    return {};
  }) };
  return { api, location, frame: () => { framed = true; } };
}

it("fills only native-selected fields in an isolated main-frame world and clears them without returning values", async () => {
  const h = fixture(), fields = await prepareNativeLogin(h.api, h.location.origin, () => {});
  const secret = { username: "synthetic@example.invalid", password: "Synthetic Private Password" };
  await fillNativeLogin(h.api, fields, secret, () => {});
  expect([...document.querySelectorAll("input")].map((input) => input.value)).toEqual([secret.username, secret.password]);
  expect(h.api.sendCommand).toHaveBeenCalledWith("Page.createIsolatedWorld", { frameId: "main", worldName: "Tjommie private login", grantUniveralAccess: false });
  const fillCall = h.api.sendCommand.mock.calls.find(([method]) => method === "Runtime.callFunctionOn")!;
  expect(String(fillCall[1]?.functionDeclaration)).not.toContain(secret.password);
  await clearNativeLogin(h.api, fields, () => {}); expect([...document.querySelectorAll("input")].every((input) => input.value === "")).toBe(true);
});

it.each([
  '<form method="get"><input autocomplete="username"><input type="password"></form>',
  '<form method="post" action="https://elsewhere.invalid"><input autocomplete="username"><input type="password"></form>',
  '<form method="post" action="https://login.invalid"><input autocomplete="username"><input type="password"><input type="password"></form>',
  '<form method="post" action="https://login.invalid"><input autocomplete="username"><input type="password"><iframe></iframe></form>',
])("refuses ambiguous, insecure or framed forms before a secret is supplied: %s", async (html) => {
  const h = fixture(html); await expect(prepareNativeLogin(h.api, h.location.origin, () => {})).rejects.toThrow("cannot receive");
  expect(h.api.sendCommand.mock.calls.some(([method]) => method === "Runtime.callFunctionOn")).toBe(false);
});

it("refuses navigation, unexpected frames, and replaced input nodes after preparation", async () => {
  const h = fixture(), fields = await prepareNativeLogin(h.api, h.location.origin, () => {}), secret = { username: "synthetic", password: "Synthetic Value" };
  h.location.href = "https://other.invalid/login";
  await expect(fillNativeLogin(h.api, fields, secret, () => {})).rejects.toThrow("cannot receive");
  h.location.href = "https://login.invalid/signin";
  document.querySelector('input[type="password"]')!.replaceWith(document.createElement("input"));
  await expect(fillNativeLogin(h.api, fields, secret, () => {})).rejects.toThrow("cannot receive");
  h.frame(); await expect(prepareNativeLogin(h.api, h.location.origin, () => {})).rejects.toThrow("cannot receive");
  expect([...document.querySelectorAll("input")].every((input) => input.value === "")).toBe(true);
});
