import { daemon, request } from "./connection";
import { GOOGLE_SERVICES, type GoogleApi } from "./google-connection";
import { localFileActions } from "./native-capability";

// The Google card's calls, on the live daemon client. Connecting is the desktop app's: it begins the sign-in and opens
// Google in the system browser, and returns the attempt. It asks for full access to Gmail, Drive and Calendar. The desktop
// app begins it on this PC's daemon, so only a window showing this PC connects.
export const googleApi: GoogleApi = {
  status: () => request("google.status", {}),
  configure: (clientId, clientSecret) => request("google.configure", { clientId, clientSecret }),
  connect: async () => {
    const api = localFileActions();
    if (!api) throw new Error("Connect Google in Tjommie on the PC it's for.");
    return api.connectGoogle(GOOGLE_SERVICES);
  },
  cancel: (attempt) => request("google.cancel", { attempt }),
  disconnect: () => request("google.disconnect", {}),
  grant: (botId, enabled) => request("google.grant", { botId, enabled }),
  // The client that is live now; the card asks again when the connection is replaced.
  onChanged: (listener) => {
    try {
      return daemon().subscribe((event) => {
        if (event.event === "google.changed") listener(event.data);
      });
    } catch {
      return () => {};
    }
  },
};
