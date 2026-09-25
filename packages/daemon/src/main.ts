import { startDaemon } from "./daemon.ts";
import { DEFAULT_PORT } from "./paths.ts";

const port = Number(process.env.TJOMMIE_PORT ?? DEFAULT_PORT);
try {
  const daemon = await startDaemon({ port });
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
  throw error;
}
