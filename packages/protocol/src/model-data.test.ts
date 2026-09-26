import { describe, expect, expectTypeOf, it } from "vitest";
import {
  ActivityData,
  ApprovalData,
  ErrorData,
  ExchangePostData,
  FileData,
  HandoffData,
  Message,
  NoticeData,
  RoutineLabelData,
  RoutineRun,
  UserMessageData,
  messageData,
} from "./index.ts";

const message = (kind: Message["kind"], data: Message["data"]): Message => ({
  id: "message",
  conversationId: "chat",
  author: "bot",
  kind,
  body: "Content",
  data,
  status: "done",
  createdAt: "2026-09-26T12:00:00.000Z",
});

describe("message payload contract", () => {
  it("validates exchange handoffs and room answer links", () => {
    const exchange = { exchangeId: "exchange", fromBotId: "writer", toBotId: "reader", messages: 2, status: "replied" };
    expect(HandoffData.parse(exchange)).toEqual(exchange);
    expect(HandoffData.parse({ roomMessageId: "room-reply" })).toEqual({ roomMessageId: "room-reply" });
    expect(() => HandoffData.parse({ ...exchange, status: "done" })).toThrow();
    expect(() => HandoffData.parse({ ...exchange, messages: "2" })).toThrow();
    expect(() => HandoffData.parse({ roomMessageId: 1 })).toThrow();
  });

  it("validates routine label run status and note", () => {
    for (const status of ["queued", "running", "done", "stopped", "failed", "missed", "skipped"]) {
      const label = { routineId: "routine", runId: "run", status, note: status === "missed" ? "The PC was asleep." : null };
      expect(RoutineLabelData.parse(label)).toEqual(label);
    }
    expect(() => RoutineLabelData.parse({ routineId: "routine", runId: "run", status: "replied", note: null })).toThrow();
  });

  it("validates shared file cards", () => {
    const file = { path: "C:\\Bot\\inbox\\report.txt", name: "report.txt", size: 12 };
    expect(FileData.parse(file)).toEqual(file);
    expect(() => FileData.parse({ ...file, size: -1 })).toThrow();
    expect(() => FileData.parse({ ...file, path: null })).toThrow();
  });

  it("validates tool and routine approval cards in every persisted state", () => {
    for (const status of ["pending", "allowed", "denied"]) {
      const tool = { approvalId: "approval", tool: "Write", detail: "Write a file", rule: null, status };
      const routine = { approvalId: "proposal", kind: "routine", tool: "Routine", detail: "Check each morning", status };
      expect(ApprovalData.parse(tool)).toEqual(tool);
      expect(ApprovalData.parse({ ...tool, kind: "tool" })).toEqual({ ...tool, kind: "tool" });
      expect(ApprovalData.parse(routine)).toEqual(routine);
    }
    expect(() => ApprovalData.parse({ approvalId: "approval", tool: "Write", detail: "Write", rule: null, status: "done" })).toThrow();
  });

  it("validates activity items and their progress", () => {
    const activity = { items: [
      { id: "a", name: "Read", summary: "Read a file", status: "running" },
      { id: "b", name: "Write", summary: "Write a file", status: "done" },
      { id: "c", name: "Bash", summary: "Run a command", status: "failed" },
    ] };
    expect(ActivityData.parse(activity)).toEqual(activity);
    expect(() => ActivityData.parse({ items: [{ ...activity.items[0], status: "pending" }] })).toThrow();
    expect(() => ActivityData.parse({ items: "Read a file" })).toThrow();
  });

  it("validates retry links and exchange failure notice links", () => {
    expect(ErrorData.parse({ retryMessageId: "failed-message" })).toEqual({ retryMessageId: "failed-message" });
    expect(ErrorData.parse({ retryMessageId: "room-post", retryBotId: "failed-bot" })).toEqual({ retryMessageId: "room-post", retryBotId: "failed-bot" });
    expect(NoticeData.parse({ exchangeId: "failed-exchange" })).toEqual({ exchangeId: "failed-exchange" });
    expect(() => ErrorData.parse({ retryMessageId: false })).toThrow();
    expect(() => ErrorData.parse({ retryMessageId: "room-post", retryBotId: false })).toThrow();
    expect(() => NoticeData.parse({ exchangeId: 1 })).toThrow();
  });

  it("distinguishes copied attachment names from exchange file paths", () => {
    const attachments = { files: ["report.txt", "notes.txt"] };
    const exchange = { exchangeId: "exchange", toBotId: "reader", files: [{ name: "report.txt", path: "C:\\Bot\\inbox\\report.txt" }] };
    expect(UserMessageData.parse(attachments)).toEqual(attachments);
    expect(ExchangePostData.parse(exchange)).toEqual(exchange);
    expect(ExchangePostData.parse({ exchangeId: "exchange", toBotId: "reader" })).toEqual({ exchangeId: "exchange", toBotId: "reader" });
    expect(() => UserMessageData.parse({ files: exchange.files })).toThrow();
    expect(() => ExchangePostData.parse({ ...exchange, files: attachments.files })).toThrow();
  });

  it("keeps the raw wire data compatible while validating data at the typed reader", () => {
    const legacy = message("text", { futureField: true });
    expect(Message.parse(legacy).data).toEqual({ futureField: true });
    expect(() => messageData(legacy)).toThrow();
    expect(Message.parse(message("text", null)).data).toBeNull();
  });

  it("returns data narrowed by the message kind", () => {
    const parsed = messageData(message("routine", { routineId: "routine", runId: "run", status: "done", note: null }));
    if (parsed.kind !== "routine") throw new Error("Expected a routine label");
    expectTypeOf(parsed.data).toEqualTypeOf<RoutineLabelData>();
    expect(parsed.data.runId).toBe("run");

    const text = messageData(message("text", { exchangeId: "exchange", toBotId: "reader" }));
    if (text.kind !== "text" || text.data === null || !("toBotId" in text.data)) throw new Error("Expected an exchange post");
    expectTypeOf(text.data).toEqualTypeOf<ExchangePostData>();
    expect(text.data.toBotId).toBe("reader");
  });

  it("validates each message kind instead of accepting another kind's payload", () => {
    const cases: { kind: Message["kind"]; data: Message["data"] }[] = [
      { kind: "text", data: { files: ["report.txt"] } },
      { kind: "activity", data: { items: [] } },
      { kind: "approval", data: { approvalId: "a", tool: "Write", detail: "Write", rule: null, status: "pending" } },
      { kind: "notice", data: { exchangeId: "exchange" } },
      { kind: "error", data: { retryMessageId: "retry" } },
      { kind: "handoff", data: { roomMessageId: "reply" } },
      { kind: "routine", data: { routineId: "routine", runId: "run", status: "done", note: null } },
      { kind: "file", data: { path: "C:\\Bot\\inbox\\report.txt", name: "report.txt", size: 12 } },
    ];
    for (const row of cases) {
      expect(messageData(message(row.kind, row.data))).toEqual(row);
      expect(() => messageData(message(row.kind, { invalidField: true }))).toThrow();
    }
    expect(() => messageData(message("file", { retryMessageId: "retry" }))).toThrow();
  });

  it("permits absent data only on plain text, notices and errors", () => {
    for (const kind of ["text", "notice", "error"] as const) {
      expect(messageData(message(kind, null))).toEqual({ kind, data: null });
    }
    for (const kind of ["activity", "approval", "handoff", "routine", "file"] as const) {
      expect(() => messageData(message(kind, null))).toThrow();
    }
  });
});

describe("routine run contract", () => {
  it("carries the owning bot id", () => {
    const run = { id: "run", botId: "bot", routineId: "routine", dueAt: "due", startedAt: null, endedAt: null, status: "queued", note: null };
    expect(RoutineRun.parse(run)).toEqual(run);
    const { botId: _botId, ...withoutBot } = run;
    expect(() => RoutineRun.parse(withoutBot)).toThrow();
  });
});
