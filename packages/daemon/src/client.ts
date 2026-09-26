import { PROTOCOL_VERSION, events, methods, type EventData, type EventName, type MethodName, type ParamsIn, type Result } from "@tjommie/protocol";

export type ClientEvent = { [E in EventName]: { type: "event"; event: E; data: EventData<E> } }[EventName];
export type Client = {
  call: <M extends MethodName>(method: M, params: ParamsIn<M>) => Promise<Result<M>>;
  events: ClientEvent[];
  subscribe: (listener: (event: ClientEvent) => void) => () => void;
  disconnected: Promise<ClientConnectionError>;
  close: () => Promise<void>;
};

type PendingCall = { accept: (result: unknown) => void; reject: (error: Error) => void };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export class ClientConnectionError extends Error {
  readonly code: number;
  readonly reason: string;
  readonly opened: boolean;

  constructor(code: number, reason: string, opened: boolean) {
    super(`WebSocket closed (code ${code}, reason: ${reason || "none"})`);
    this.name = "ClientConnectionError";
    this.code = code;
    this.reason = reason;
    this.opened = opened;
  }
}

// Node's built-in WebSocket speaks the same protocol as the desktop client.
export async function connect(port: number, token: string): Promise<Client> {
  const socket = new WebSocket(`ws://127.0.0.1:${port}`);
  const received: ClientEvent[] = [];
  const subscribers = new Set<(event: ClientEvent) => void>();
  const pending = new Map<number, PendingCall>();
  let nextId = 1;
  let opened = false;
  let welcomed = false;
  let failure: ClientConnectionError | undefined;
  let resolveWelcome: () => void = () => {};
  let rejectWelcome: (error: Error) => void = () => {};
  const welcome = new Promise<void>((resolve, reject) => {
    resolveWelcome = resolve;
    rejectWelcome = reject;
  });
  let resolveClosed: () => void = () => {};
  const closed = new Promise<void>((resolve) => { resolveClosed = resolve; });
  let resolveDisconnected: (error: ClientConnectionError) => void = () => {};
  const disconnected = new Promise<ClientConnectionError>((resolve) => { resolveDisconnected = resolve; });

  function connectionError(code: number, reason: string): ClientConnectionError {
    return new ClientConnectionError(code, reason, opened);
  }

  function fail(error: ClientConnectionError): void {
    if (failure) return;
    failure = error;
    clearTimeout(helloTimer);
    resolveDisconnected(error);
    rejectWelcome(error);
    for (const call of pending.values()) call.reject(error);
    pending.clear();
  }

  function invalidFrame(reason: string): void {
    fail(connectionError(4400, `Invalid server frame: ${reason}`));
    socket.close(4400, "Invalid server frame");
  }

  const helloTimer = setTimeout(() => {
    fail(connectionError(4408, "Welcome timed out"));
    socket.close();
  }, 5000);

  socket.addEventListener("open", () => {
    opened = true;
    if (failure) return;
    try {
      socket.send(JSON.stringify({ type: "hello", protocol: PROTOCOL_VERSION, token }));
    } catch {
      fail(connectionError(1006, "WebSocket error sending hello"));
      socket.close();
    }
  });
  socket.addEventListener("close", (event) => {
    fail(connectionError(event.code, event.reason));
    resolveClosed();
  });
  socket.addEventListener("error", () => {
    fail(connectionError(1006, "WebSocket error"));
  });
  socket.addEventListener("message", (event) => {
    if (failure) return;
    let frame: unknown;
    try {
      if (typeof event.data !== "string") throw new Error("Expected a text frame");
      frame = JSON.parse(event.data);
      if (!isRecord(frame)) throw new Error("Expected an envelope");
      if (frame.type === "welcome") {
        if (welcomed || frame.protocol !== PROTOCOL_VERSION) throw new Error("Unexpected welcome protocol");
        welcomed = true;
        clearTimeout(helloTimer);
        resolveWelcome();
        return;
      }
      if (!welcomed) throw new Error("Expected welcome first");
      if (frame.type === "res") {
        if (typeof frame.id !== "number" || !Number.isSafeInteger(frame.id) || typeof frame.ok !== "boolean") throw new Error("Invalid response envelope");
        const call = pending.get(frame.id);
        if (frame.ok) {
          if (!("result" in frame)) throw new Error("Missing response result");
          call?.accept(frame.result);
        } else {
          if (!isRecord(frame.error) || typeof frame.error.code !== "string" || typeof frame.error.message !== "string") throw new Error("Invalid response error");
          call?.reject(new Error(`${frame.error.code}: ${frame.error.message}`));
        }
        pending.delete(frame.id);
        return;
      }
      if (frame.type === "event") {
        if (typeof frame.event !== "string" || !Object.hasOwn(events, frame.event) || !("data" in frame)) throw new Error("Invalid event envelope");
        const name = frame.event as EventName;
        // The name selects the matching schema, preserving the event/data pairing.
        const message = { type: "event", event: name, data: events[name].parse(frame.data) } as ClientEvent;
        received.push(message);
        for (const subscriber of subscribers) {
          try { subscriber(message); }
          catch (error: unknown) { console.error(error); }
        }
        return;
      }
      throw new Error("Unknown envelope type");
    } catch (error: unknown) {
      invalidFrame(error instanceof Error ? error.message : "Could not parse message");
    }
  });

  await welcome;

  function call<M extends MethodName>(method: M, params: ParamsIn<M>): Promise<Result<M>> {
    return new Promise<Result<M>>((resolve, reject) => {
      if (failure) { reject(failure); return; }
      if (socket.readyState !== WebSocket.OPEN) { reject(connectionError(1006, "Socket is not open")); return; }
      const id = nextId++;
      pending.set(id, {
        // This schema is selected by the method captured for this pending call.
        accept: (result) => resolve(methods[method].result.parse(result) as Result<M>),
        reject,
      });
      try {
        socket.send(JSON.stringify({ type: "req", id, method, params }));
      } catch (error: unknown) {
        pending.delete(id);
        reject(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  return {
    call,
    events: received,
    disconnected,
    subscribe: (listener) => {
      subscribers.add(listener);
      return () => { subscribers.delete(listener); };
    },
    close: async () => {
      if (socket.readyState === WebSocket.CLOSED) return;
      fail(connectionError(1000, "Client closed"));
      socket.close(1000, "Client closed");
      await closed;
    },
  };
}
