import { createInterface } from "node:readline";
import { PlainStartupError, startDaemon } from "./daemon.ts";
import { DEFAULT_PORT } from "./paths.ts";

const noModeMessage = "Start Tjommie from the desktop app, or run npm start -w @tjommie/daemon for development.";

function tokenFromStdin(): Promise<string> {
  return new Promise((resolve, reject) => {
    const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
    let done = false;
    const finish = (line?: string, missingMessage = "No daemon token arrived on stdin within 5 seconds."): void => {
      if (done) return;
      done = true;
      clearTimeout(timeout);
      lines.close();
      process.stdin.pause();
      if (line === undefined) reject(new PlainStartupError(missingMessage));
      else resolve(line);
    };
    const timeout = setTimeout(() => finish(), 5_000);
    lines.once("line", finish);
    lines.once("close", () => finish(undefined, "stdin closed before a daemon token arrived."));
  });
}

const port = Number(process.env.TJOMMIE_PORT ?? DEFAULT_PORT);
try {
  const args = process.argv.slice(2);
  const development = process.env.TJOMMIE_DEV === "1" || args.includes("--dev");
  const production = args.includes("--token-stdin");
  const unknown = args.find((arg) => arg !== "--dev" && arg !== "--token-stdin");
  if (unknown !== undefined) throw new PlainStartupError(`Unknown argument: ${unknown}`);
  if (development === production) throw new PlainStartupError(development ? "Choose either --dev or --token-stdin." : noModeMessage);
  const credentials = development ? null : await tokenFromStdin();
  let productionAuth: { mode: "production"; token: string; desktopHostToken?: string } = { mode: "production", token: credentials ?? "" };
  if (credentials?.startsWith("{")) {
    let value: unknown;
    try { value = JSON.parse(credentials); } catch { throw new PlainStartupError("Invalid desktop startup credentials."); }
    if (typeof value !== "object" || value === null || !("token" in value) || typeof value.token !== "string" || !("desktopHostToken" in value) || typeof value.desktopHostToken !== "string" || !/^[A-Za-z0-9_-]{43,}$/.test(value.desktopHostToken)) throw new PlainStartupError("Invalid desktop startup credentials.");
    productionAuth = { mode: "production", token: value.token, desktopHostToken: value.desktopHostToken };
  }
  const auth = development
    ? { mode: "development" as const }
    : productionAuth;
  // Test hook: the desktop app's real-CLI tests pair a disposable data root with the signed-in account folders.
  const accountsDir = process.env.TJOMMIE_ACCOUNTS_DIR || undefined;
  if (accountsDir !== undefined) console.error("Using account folders from TJOMMIE_ACCOUNTS_DIR.");
  const daemon = await startDaemon({ port, auth, accountsDir });
  console.log(`Tjommie daemon listening on http://127.0.0.1:${daemon.port}`);
  let stopping = false;
  const stop = (): void => {
    if (stopping) return;
    stopping = true;
    void daemon
      .close()
      .catch((error: unknown) => console.error("Failed to stop the Tjommie daemon cleanly.", error))
      .finally(() => process.exit(0));
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
  if (production) {
    // The desktop app keeps the other end of stdin open for as long as it runs. When it quits or crashes the
    // pipe closes and the daemon stops too, so no daemon is left holding the port with a token nobody has.
    process.stdin.once("end", stop);
    process.stdin.once("error", stop);
    process.stdin.resume();
    if (process.stdin.readableEnded) stop(); // it closed while the daemon was starting
  }
} catch (error) {
  if (error && typeof error === "object" && "code" in error && error.code === "EADDRINUSE") {
    console.error(`Tjommie is already running: port ${port} is in use.`);
    process.exit(1);
  }
  if (error instanceof PlainStartupError) console.error(error.message);
  else console.error(error);
  process.exit(1);
}
