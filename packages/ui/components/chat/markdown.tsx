"use client";

import { createContext, useContext, type ComponentProps, type ReactNode } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

// The desktop app opens web links in the default browser and blocks every other target.
const web = (url: unknown): url is string => typeof url === "string" && /^https?:/i.test(url);
const InLink = createContext(false);

// A link that can open. Anything else (a target dropped as unsafe, an email address) shows as plain text.
function WebLink({ href, children }: { href?: string; children?: ReactNode }) {
  if (!web(href)) return <>{children}</>;
  return (
    <InLink value={true}>
      <a href={href} target="_blank" rel="noreferrer">
        {children}
      </a>
    </InLink>
  );
}

// Images never load. A web image becomes a link to it (just its alt text inside another link); any other image
// shows its alt text.
function MarkdownImage({ src, alt }: ComponentProps<"img">) {
  const inLink = useContext(InLink);
  return web(src) && !inLink ? <WebLink href={src}>{alt || src}</WebLink> : <>{alt}</>;
}

// Bot replies are Markdown. Raw HTML never renders, unsafe link targets are dropped, and links open in the
// default browser (the desktop app sends every new window there).
export function MarkdownText({ text }: { text: string }) {
  return (
    <div className="markdown">
      <Markdown remarkPlugins={[remarkGfm]} components={{ a: WebLink, img: MarkdownImage }}>
        {text}
      </Markdown>
    </div>
  );
}
