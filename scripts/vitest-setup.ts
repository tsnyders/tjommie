import { DEFAULT_RELAY_URL } from "@tjommie/protocol";
import { afterEach, expect } from "vitest";

if (DEFAULT_RELAY_URL === null) throw new Error("The public relay URL must be set before running tests.");
const normalizedHostname = (hostname: string): string => hostname.toLowerCase().replace(/\.$/, "");
const productionRelayHostname = normalizedHostname(new URL(DEFAULT_RELAY_URL).hostname);
type NetworkTarget = Parameters<typeof fetch>[0] | ConstructorParameters<typeof WebSocket>[0];
type RelayViolation = { method: "fetch" | "WebSocket"; host: string };
const relayViolations: RelayViolation[] = [];

export function clearRelayViolations(): void {
  relayViolations.length = 0;
}

function blockProductionRelay(target: NetworkTarget, method: "fetch" | "WebSocket"): void {
  if (process.env.TJOMMIE_ALLOW_PRODUCTION_RELAY === "1") return;
  const address = typeof target === "string" ? target : target instanceof URL ? target.href : target.url;
  let url: URL;
  try { url = new URL(address); } catch { return; }
  if (normalizedHostname(url.hostname).endsWith(".workers.dev") || normalizedHostname(url.hostname) === productionRelayHostname) {
    const testName = expect.getState().currentTestName ?? "unknown test";
    relayViolations.push({ method, host: url.host });
    throw new Error(`Blocked ${method} to production relay host ${url.host} in test "${testName}". Set TJOMMIE_ALLOW_PRODUCTION_RELAY=1 only for the production smoke test.`);
  }
}

afterEach(() => {
  const violations = relayViolations.splice(0);
  if (violations.length === 0) return;
  const testName = expect.getState().currentTestName ?? "unknown test";
  const targets = violations.map(({ method, host }) => `${method} ${host}`).join(", ");
  throw new Error(`Test "${testName}" attempted blocked production relay access: ${targets}.`);
});

const nativeFetch = globalThis.fetch.bind(globalThis);
globalThis.fetch = async (...args) => {
  blockProductionRelay(args[0], "fetch");
  return nativeFetch(...args);
};

const NativeWebSocket = globalThis.WebSocket;
class GuardedWebSocket extends NativeWebSocket {
  constructor(...args: ConstructorParameters<typeof NativeWebSocket>) {
    blockProductionRelay(args[0], "WebSocket");
    super(...args);
  }
}
globalThis.WebSocket = GuardedWebSocket;
