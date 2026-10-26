import { expect, it } from "vitest";
import { DEFAULT_RELAY_URL } from "./relay.ts";
import { clearRelayViolations } from "../../../scripts/vitest-setup.ts";

const allowProductionRelay = process.env.TJOMMIE_ALLOW_PRODUCTION_RELAY === "1";

it.skipIf(allowProductionRelay)("blocks production relay network calls and names the test", async () => {
  if (DEFAULT_RELAY_URL === null) throw new Error("The public relay URL must be set");
  const testName = expect.getState().currentTestName;
  if (!testName) throw new Error("Vitest did not provide the current test name");
  const websocketUrl = new URL("wss://other-account.WORKERS.DEV./v1/hosts/test/connect");
  try {
    expect(() => new WebSocket(websocketUrl)).toThrow(`Blocked WebSocket to production relay host ${websocketUrl.host} in test "${testName}"`);
    await expect(fetch(DEFAULT_RELAY_URL)).rejects.toThrow(`Blocked fetch to production relay host ${new URL(DEFAULT_RELAY_URL).host} in test "${testName}"`);
  } finally {
    clearRelayViolations();
  }
});

it.skipIf(allowProductionRelay)("fails even when a production relay WebSocket error is caught", { fails: true }, () => {
  if (DEFAULT_RELAY_URL === null) throw new Error("The public relay URL must be set");
  const url = new URL(DEFAULT_RELAY_URL);
  url.protocol = "wss:";
  try { new WebSocket(url); } catch { /* RelayClient catches connection errors before retrying. */ }
});
