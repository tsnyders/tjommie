import { LoginOrigin } from "../../protocol/src/vault.ts";
import type { VaultSecret } from "./vault-storage.ts";

type Debugger = { sendCommand(method: string, params?: Record<string, unknown>): Promise<unknown> };
export type NativeLoginFields = { frameId: string; objectId: string; origin: string };
const failure = (): Error => new Error("This page cannot receive this login safely. Use an ordinary HTTPS login form in the main page.");
const object = (value: unknown): Record<string, unknown> => {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw failure(); return value as Record<string, unknown>;
};
const formCheck = String.raw`
function eligible() {
  if (document.querySelector('iframe,frame,object,embed')) return null;
  const passwords = [...document.querySelectorAll('input[type="password"]')];
  if (passwords.length !== 1) return null;
  const p = passwords[0], form = p.form;
  if (!form || p.autocomplete === 'new-password' || form.method.toLowerCase() !== 'post' || form.target && form.target !== '_self') return null;
  if (new URL(form.action, location.href).origin !== location.origin) return null;
  for (const submit of form.querySelectorAll('[formaction],[formmethod],[formtarget]')) {
    if (submit.formAction && new URL(submit.formAction, location.href).origin !== location.origin || submit.formMethod && submit.formMethod.toLowerCase() !== 'post' || submit.formTarget && submit.formTarget !== '_self') return null;
  }
  let candidates = [...form.elements].filter(n => n instanceof HTMLInputElement && ['text','email'].includes(n.type) && (n.autocomplete === 'username' || n.type === 'email'));
  if (!candidates.length) candidates = [...form.elements].filter(n => n instanceof HTMLInputElement && ['text','email'].includes(n.type));
  if (candidates.length !== 1) return null;
  const u = candidates[0];
  for (const n of [u,p]) {
    const r = n.getBoundingClientRect(), style = getComputedStyle(n);
    if (!n.isConnected || n.disabled || n.readOnly || r.width <= 0 || r.height <= 0 || style.display === 'none' || style.visibility !== 'visible') return null;
  }
  return {u,p,form,url:location.href};
}`;
const prepare = `(() => { ${formCheck} return eligible(); })()`;
const fill = `function(username,password,origin) {
  ${formCheck}
  const now = eligible();
  if (location.origin !== origin || location.href !== this.url || !now || now.u !== this.u || now.p !== this.p || now.form !== this.form) return false;
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;
  set.call(this.u,username); set.call(this.p,password);
  for (const n of [this.u,this.p]) { n.dispatchEvent(new Event('input',{bubbles:true})); n.dispatchEvent(new Event('change',{bubbles:true})); }
  return true;
}`;
const clear = `function() {
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;
  for (const n of [this.u,this.p]) { set.call(n,''); if(n.isConnected) n.dispatchEvent(new Event('input',{bubbles:true})); }
  return this.u.value === '' && this.p.value === '';
}`;

async function frame(debuggerApi: Debugger, origin: string): Promise<string> {
  LoginOrigin.parse(origin);
  const tree = object(object(await debuggerApi.sendCommand("Page.getFrameTree")).frameTree);
  if (Array.isArray(tree.childFrames) && tree.childFrames.length) throw failure();
  const main = object(tree.frame);
  if (typeof main.id !== "string" || typeof main.url !== "string") throw failure();
  const url = new URL(main.url);
  if (url.protocol !== "https:" || url.origin !== origin || url.username || url.password) throw failure();
  return main.id;
}
export async function prepareNativeLogin(debuggerApi: Debugger, origin: string, check: () => void): Promise<NativeLoginFields> {
  check(); const frameId = await frame(debuggerApi, origin); check();
  const world = object(await debuggerApi.sendCommand("Page.createIsolatedWorld", { frameId, worldName: "Tjommie private login", grantUniveralAccess: false })); check();
  if (typeof world.executionContextId !== "number") throw failure();
  const value = object(await debuggerApi.sendCommand("Runtime.evaluate", { expression: prepare, contextId: world.executionContextId, returnByValue: false })); check();
  const result = object(value.result);
  if (value.exceptionDetails || typeof result.objectId !== "string" || result.subtype === "null") throw failure();
  if (await frame(debuggerApi, origin) !== frameId) throw failure(); check();
  return { frameId, objectId: result.objectId, origin };
}
export async function fillNativeLogin(debuggerApi: Debugger, fields: NativeLoginFields, secret: VaultSecret, check: () => void): Promise<void> {
  check(); if (await frame(debuggerApi, fields.origin) !== fields.frameId) throw failure(); check();
  const answer = object(await debuggerApi.sendCommand("Runtime.callFunctionOn", { objectId: fields.objectId, functionDeclaration: fill,
    arguments: [{ value: secret.username }, { value: secret.password }, { value: fields.origin }], returnByValue: true, silent: true }));
  check(); if (answer.exceptionDetails || object(answer.result).value !== true) throw failure();
}
export async function clearNativeLogin(debuggerApi: Debugger, fields: NativeLoginFields, check: () => void): Promise<void> {
  check();
  const answer = object(await debuggerApi.sendCommand("Runtime.callFunctionOn", { objectId: fields.objectId, functionDeclaration: clear, returnByValue: true, silent: true })); check();
  if (answer.exceptionDetails || object(answer.result).value !== true) throw new Error("Logins could not verify cleared fields. Close this private browser before resuming.");
  await debuggerApi.sendCommand("Runtime.releaseObject", { objectId: fields.objectId }); check();
}
