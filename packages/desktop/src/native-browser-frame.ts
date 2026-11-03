type Debugger = {
  sendCommand(method: string, parameters?: Record<string, unknown>): Promise<unknown>;
  on(event: "message", listener: (event: unknown, method: string, parameters: unknown) => void): unknown;
  removeListener(event: "message", listener: (event: unknown, method: string, parameters: unknown) => void): unknown;
};
const record = (value: unknown): Record<string, unknown> => value && typeof value === "object" ? value as Record<string, unknown> : {};
const IMAGE_LIMIT = 2 * 1024 * 1024;

/** A scoped screencast makes a hidden native view paint; no screencast image is retained. */
export async function captureNativeBrowserFrame(debuggerApi: Debugger, check: () => void, parameters: Record<string, unknown> = { format: "jpeg", quality: 60, captureBeyondViewport: false }): Promise<string> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const expired = new Promise<never>((_resolve, reject) => { timer = setTimeout(() => reject(new Error("The native browser frame timed out.")), 5000); });
  const send = async (method: string, args?: Record<string, unknown>): Promise<Record<string, unknown>> => {
    check(); const value = record(await Promise.race([debuggerApi.sendCommand(method, args), expired])); check(); return value;
  };
  const ack = (_event: unknown, method: string, parameters: unknown): void => {
    if (method !== "Page.screencastFrame") return;
    const id = record(parameters).sessionId;
    if (typeof id === "number") void debuggerApi.sendCommand("Page.screencastFrameAck", { sessionId: id }).catch(() => {});
  };
  check(); debuggerApi.on("message", ack);
  let captured: string;
  try {
    await send("Page.startScreencast", { format: "jpeg", quality: 60, maxWidth: 2560, maxHeight: 1600, everyNthFrame: 1 });
    const result = await send("Page.captureScreenshot", parameters), data = result.data;
    if (typeof data !== "string" || !/^[A-Za-z0-9+/]+={0,2}$/.test(data) || data.length > Math.ceil(IMAGE_LIMIT / 3) * 4 || Buffer.byteLength(data, "base64") > IMAGE_LIMIT) throw new Error("The browser frame was invalid or exceeded 2 MiB.");
    captured = data;
  } finally {
    clearTimeout(timer); debuggerApi.removeListener("message", ack);
    let cleanup: ReturnType<typeof setTimeout> | undefined;
    try { await Promise.race([debuggerApi.sendCommand("Page.stopScreencast").catch(() => {}), new Promise<void>((resolve) => { cleanup = setTimeout(resolve, 500); })]); }
    finally { clearTimeout(cleanup); }
  }
  check(); return captured;
}
