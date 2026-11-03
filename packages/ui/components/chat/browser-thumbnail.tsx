"use client";
/* eslint-disable @next/next/no-img-element -- live browser frames are not static assets */

import { useEffect, useRef, useState } from "react";
import type { BrowserFrame } from "@tjommie/protocol";
import { BrowserPreview } from "@/lib/browser-preview";
import { request } from "@/lib/connection";
import { useApp } from "@/lib/store";

export function BrowserThumbnail({ botId, name }: { botId: string; name: string }) {
  const host = useApp((state) => state.host), ready = useApp((state) => state.connection.status === "ready");
  const element = useRef<HTMLDivElement>(null), [frame, setFrame] = useState<BrowserFrame | null>(null);
  useEffect(() => {
    const preview = new BrowserPreview(() => request("browser.frame", { botId, thumbnail: true }), setFrame);
    let intersects = false;
    const visibility = (): void => preview.setVisible(ready && intersects && document.visibilityState !== "hidden");
    const observer = new IntersectionObserver(([entry]) => { intersects = entry.isIntersecting; visibility(); });
    if (element.current) observer.observe(element.current);
    document.addEventListener("visibilitychange", visibility);
    return () => { observer.disconnect(); document.removeEventListener("visibilitychange", visibility); preview.setVisible(false); };
  }, [botId, host, ready]);
  return <div ref={element} data-slot="browser-thumbnail" className="relative aspect-video w-full overflow-hidden rounded-lg bg-canvas">
    {frame ? <img src={`data:${frame.mime};base64,${frame.data}`} alt={`${name}'s browser preview`} className="size-full object-contain" /> : <span className="absolute inset-0 flex items-center justify-center text-[12px] text-secondary">Browser preview</span>}
  </div>;
}
