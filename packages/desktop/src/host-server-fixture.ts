import { createServer, type IncomingMessage, type ServerResponse } from "node:http";

// What the host tests need of a daemon's HTTP side: a loopback listener that sends each private host route to its handler.
// The real daemon (tjommie-rs, native_browser_broker.rs and vault_host_broker.rs) serves /browser-host and /vault-host the
// same way: no Origin header, and the route's own bearer-token check.
export type Route = (req: IncomingMessage, res: ServerResponse) => void | Promise<void>;
export type HostServer = { port: number; close(): Promise<void> };

export async function startHostServer(routes: Record<string, Route>): Promise<HostServer> {
  const http = createServer((req, res) => {
    const route = routes[(req.url ?? "").split("?")[0]];
    if (!route) { res.writeHead(404).end(); return; }
    if (req.headers.origin !== undefined) { res.writeHead(403).end(); return; }
    Promise.resolve().then(() => route(req, res)).catch((error: unknown) => {
      console.error(error);
      if (!res.headersSent) res.writeHead(500);
      res.end();
    });
  });
  await new Promise<void>((resolve) => http.listen(0, "127.0.0.1", () => resolve()));
  const address = http.address();
  if (!address || typeof address === "string") throw new Error("The host fixture did not bind a TCP port.");
  return {
    port: address.port,
    close: () => new Promise<void>((resolve) => { http.close(() => resolve()); http.closeAllConnections(); }),
  };
}
