import { describe, expect, it } from "vitest";
import { GoogleClientConfig, GoogleStatus, googleMethods } from "./google.ts";

describe("Google connection protocol", () => {
  it("accepts write-only Desktop client settings and rejects arbitrary auth URLs", () => {
    expect(GoogleClientConfig.parse({ clientId: "123-test.apps.googleusercontent.com", clientSecret: "synthetic" })).toHaveProperty("clientSecret", "synthetic");
    expect(GoogleClientConfig.safeParse({ clientId: "https://evil.test", clientSecret: "synthetic" }).success).toBe(false);
    expect(GoogleClientConfig.safeParse({ clientId: "123-test.apps.googleusercontent.com", clientSecret: "synthetic", tokenUrl: "https://evil.test" }).success).toBe(false);
  });
  it("keeps credentials and authorization URLs out of status", () => {
    const status = { configured: true, connected: true, email: "fixture@example.test", services: ["gmail"], grantedBotIds: ["one"], attempt: null };
    expect(GoogleStatus.parse(status)).toEqual(status);
    for (const key of ["accessToken", "refreshToken", "clientSecret", "url"]) expect(GoogleStatus.safeParse({ ...status, [key]: "synthetic" }).success).toBe(false);
  });
  it("requires explicit unique read-service selection and an explicit bot grant", () => {
    expect(googleMethods["google.begin"].params.parse({ services: ["gmail"] })).toEqual({ services: ["gmail"] });
    for (const services of [[], ["gmail", "gmail"], ["gmail.send"], ["calendar.write"]]) expect(googleMethods["google.begin"].params.safeParse({ services }).success).toBe(false);
    expect(googleMethods["google.grant"].params.safeParse({ botId: "one" }).success).toBe(false);
    expect(googleMethods["google.grant"].params.parse({ botId: "one", enabled: false })).toEqual({ botId: "one", enabled: false });
  });
});
