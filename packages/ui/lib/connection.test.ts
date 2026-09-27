import { describe, expect, it } from "vitest";
import { request } from "./connection";

describe("connection", () => {
  it("rejects a call made while Tjommie isn't connected, instead of throwing", async () => {
    await expect(request("bots.list", {})).rejects.toThrow("Tjommie isn't connected right now.");
  });
});
