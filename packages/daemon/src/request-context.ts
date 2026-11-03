// Created by the authenticated transport, never parsed from request parameters.
export type RequestContext = {
  connectionId: string;
  principal: { kind: "local" } | { kind: "device"; deviceId: string };
  signal: AbortSignal;
};
