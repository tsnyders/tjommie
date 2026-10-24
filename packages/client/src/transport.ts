// A channel is supplied only after its caller has authenticated the remote peer.
// It carries RPC text inside the encrypted transport; it never receives the PC's local bearer token.
export interface RpcTransport {
  send(text: string): void;
  close(): void;
  onMessage(listener: (text: string) => void): () => void;
  onClose(listener: (reason: { code: number; reason: string }) => void): () => void;
}

export interface RpcSocket {
  readonly readyState: number;
  send(text: string): void;
  close(code?: number, reason?: string): void;
  addEventListener(type: "open" | "error", listener: () => void): void;
  addEventListener(type: "close", listener: (event: { code: number; reason: string }) => void): void;
  addEventListener(type: "message", listener: (event: { data: unknown }) => void): void;
}

export function transportSocket(transport: RpcTransport): RpcSocket {
  let readyState = 0;
  const open = new Set<() => void>(), errors = new Set<() => void>();
  const messages = new Set<(event: { data: unknown }) => void>();
  const closed = new Set<(event: { code: number; reason: string }) => void>();
  const cleanups: (() => void)[] = [];
  const end = (event: { code: number; reason: string }): void => {
    if (readyState === 3) return;
    readyState = 3;
    for (const off of cleanups.splice(0)) off();
    for (const listener of closed) listener(event);
  };
  cleanups.push(transport.onMessage((data) => { if (readyState === 1) for (const listener of messages) listener({ data }); }));
  cleanups.push(transport.onClose(end));
  queueMicrotask(() => { if (readyState === 0) { readyState = 1; for (const listener of open) listener(); } });
  return {
    get readyState() { return readyState; },
    send: (text) => { if (readyState !== 1) throw new Error("The connection is closed."); transport.send(text); },
    close: (code = 1000, reason = "Client closed") => { end({ code, reason }); transport.close(); },
    addEventListener(type: string, listener: (() => void) | ((event: { code: number; reason: string }) => void) | ((event: { data: unknown }) => void)) {
      if (type === "open") open.add(listener as () => void);
      else if (type === "error") errors.add(listener as () => void);
      else if (type === "close") closed.add(listener as (event: { code: number; reason: string }) => void);
      else if (type === "message") messages.add(listener as (event: { data: unknown }) => void);
    },
  };
}
