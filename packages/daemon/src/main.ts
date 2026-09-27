import { createInterface } from "node:readline";
import { startDaemon } from "./daemon.ts";
import { DEFAULT_PORT } from "./paths.ts";

const noModeMessage = "Start Tjommie from the desktop app, or run npm start -w @tjommie/daemon for development.";

function tokenFromStdin(): Promise<string> {
  return new Promise((resolve, reject) => {
    const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
    let done = false;
    const finish = (line?: string): void => {
      if (done) return;
      done = true;
      clearTimeout(timeout);
      lines.close();
      process.stdin.pause();
      if (line === undefined) reject(new Error("No daemon token arrived on stdin within 5 seconds."));
      else resolve(line);
    };
    const timeout = setTimeout(() => finish(), 5_000);
    lines.once("line", finish);
    lines.once("close", () => finish());
  });
}

const port = Number(process.env.TJOMMIE_PORT ?? DEFAULT_PORT);
try {
  const args = process.argv.slice(2);
  const development = process.env.TJOMMIE_DEV === "1" || args.includes("--dev");
  const production = args.includes("--token-stdin");
  if (args.some((arg) => arg !== "--dev" && arg !== "--token-stdin") || development === production) {
    throw new Error(development && production ? "Choose either --dev or --token-stdin." : noModeMessage);
  }
  const auth = development
    ? { mode: "development" as const }
    : { mode: "production" as const, token: await tokenFromStdin() };
  const daemon = await startDaemon({ port, auth });
  console.log(`Tjommie daemon listening on http://127.0.0.1:${daemon.port}`);
  const stop = (): void => {
    void daemon
      .close()
      .catch((error: unknown) => console.error("Failed to stop the Tjommie daemon cleanly.", error))
      .finally(() => process.exit(0));
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
} catch (error) {
  if ((error as NodeJS.ErrnoException).code === "EADDRINUSE") {
    console.error(`Tjommie is already running: port ${port} is in use.`);
    process.exit(1);
  }
  console.error(error instanceof Error ? error.message : "Could not start Tjommie.");
  process.exit(1);
}
