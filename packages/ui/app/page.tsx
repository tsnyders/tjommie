"use client";

import { useRouter } from "next/navigation";
import { useEffect } from "react";
import { useApp } from "@/lib/store";

// Opens the first bot's chat. First-run setup and the New bot screen replace the empty state later.
export default function Home() {
  const router = useRouter();
  const first = useApp((state) => state.bots[0]?.id);
  useEffect(() => {
    if (first) router.replace(`/chat/?bot=${encodeURIComponent(first)}`);
  }, [first, router]);
  if (first) return null;
  return (
    <div className="flex flex-1 flex-col">
      <div className="drag h-11 shrink-0" />
      <p className="m-auto text-[13px] text-secondary">No bots yet.</p>
    </div>
  );
}
