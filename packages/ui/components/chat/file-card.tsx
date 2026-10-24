"use client";

import { Download, FileText } from "lucide-react";
import type { FileData } from "@tjommie/protocol";
import { desktop } from "@/lib/desktop";
import { fileSize } from "@/lib/format";

// A file a bot shared. The button shows it selected in Explorer; the app never runs it.
export function FileCard({ file }: { file: Pick<FileData, "path" | "name"> & { size?: number } }) {
  return (
    <div data-slot="file-card" className="flex w-[264px] max-w-full items-center gap-3 rounded-[12px] bg-bubble p-3">
      <span className="flex size-8 shrink-0 items-center justify-center rounded-[8px] bg-file-tile">
        <FileText className="size-4 text-danger" aria-hidden />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px] font-semibold leading-[18px] text-title">{file.name}</span>
        {file.size !== undefined && <span className="block text-[12px] leading-[18px] text-caption">{fileSize(file.size)}</span>}
      </span>
      <button
        type="button"
        aria-label={`Show ${file.name} in its folder`}
        title="Show in folder"
        onClick={() => desktop()?.showFile(file.path)}
        className="press flex size-8 shrink-0 items-center justify-center rounded-full text-icon hover:bg-pill"
      >
        <Download className="size-4" aria-hidden />
      </button>
    </div>
  );
}
