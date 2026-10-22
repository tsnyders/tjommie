import { z } from "zod";

export const GoogleService = z.enum(["gmail", "drive", "calendar"]);
export type GoogleService = z.infer<typeof GoogleService>;
export const GoogleServices = z.array(GoogleService).min(1).max(3).refine((values) => new Set(values).size === values.length, "Choose each service once.");
export const GoogleClientConfig = z.object({
  clientId: z.string().trim().min(1).max(512).regex(/^[a-zA-Z0-9_-]+\.apps\.googleusercontent\.com$/),
  clientSecret: z.string().trim().min(1).max(4096),
}).strict();
export type GoogleClientConfig = z.infer<typeof GoogleClientConfig>;
export const GoogleStatus = z.object({
  configured: z.boolean(), connected: z.boolean(), email: z.string().nullable(),
  services: z.array(GoogleService), grantedBotIds: z.array(z.string()),
  attempt: z.object({ id: z.string(), state: z.enum(["waiting", "failed"]), message: z.string().optional() }).nullable(),
}).strict();
export type GoogleStatus = z.infer<typeof GoogleStatus>;
export const GoogleAuthorization = z.object({ attempt: z.string(), url: z.string().url() }).strict();
export type GoogleAuthorization = z.infer<typeof GoogleAuthorization>;

// Client configuration is write-only. Authorization URLs are for the trusted desktop opener;
// the UI receives the attempt ID and google.changed status, never tokens or client credentials.
export const googleMethods = {
  "google.status": { params: z.object({}), result: GoogleStatus },
  "google.configure": { params: GoogleClientConfig, result: GoogleStatus },
  "google.begin": { params: z.object({ services: GoogleServices }), result: GoogleAuthorization },
  "google.cancel": { params: z.object({ attempt: z.string().min(1).max(100) }), result: GoogleStatus },
  "google.disconnect": { params: z.object({}), result: GoogleStatus },
  "google.grant": { params: z.object({ botId: z.string().min(1).max(200), enabled: z.boolean() }), result: GoogleStatus },
} as const;
export const googleEvents = { "google.changed": GoogleStatus } as const;
