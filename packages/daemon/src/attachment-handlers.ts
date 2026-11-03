import { attachmentMethods } from "../../protocol/src/attachments.ts";
import type { z } from "zod";
import type { Attachments } from "./attachments.ts";
import type { RequestContext } from "./request-context.ts";

type AttachmentHandlers = { [M in keyof typeof attachmentMethods]: (params: z.output<(typeof attachmentMethods)[M]["params"]>, context?: RequestContext) => z.output<(typeof attachmentMethods)[M]["result"]> | Promise<z.output<(typeof attachmentMethods)[M]["result"]>> };
export function attachmentHandlers(service: Attachments): AttachmentHandlers {
  return {
    "attachments.metadata": (ref, context) => service.metadata(ref, context),
    "attachments.open": (ref, context) => service.open(ref, context),
    "attachments.read": (params, context) => service.read(params, context),
    "attachments.close": async ({ transferId }, context) => { await service.closeTransfer(transferId, context); return {}; },
  };
}
