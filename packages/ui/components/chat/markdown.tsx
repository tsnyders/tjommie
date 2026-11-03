"use client";

import { createContext, useContext, type ComponentProps, type ReactNode } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Nodes, Root } from "mdast";

// The desktop app opens web links in the default browser and blocks every other target.
const web = (url: unknown): url is string => typeof url === "string" && /^https?:/i.test(url);
const LinkTarget = createContext<string | null>(null); // the address of the link around this content, if any
const FileResources = createContext<((source: string) => string | null) | null>(null);
const FileImages = createContext<((source: string, alt: string) => ReactNode) | null>(null);
function localImage(url: string | null): boolean {
  if (!url) return false;
  try { const target = new URL(url); return (target.protocol === "app:" && target.host === "tjommie" || target.protocol === "https:" && target.host === "localhost") && !target.username && !target.password && !target.search && !target.hash && /^\/_attachments\/[0-9a-f-]{36}$/.test(target.pathname); }
  catch { return false; }
}

function fileAnchors() {
  return (tree: Root) => {
    const ids = new Map<string, number>();
    const text = (node: Nodes): string => "value" in node ? String(node.value) : "children" in node ? node.children.map(text).join("") : "";
    const visit = (node: Nodes): void => {
      if (node.type === "heading") {
        const slug = text(node).toLowerCase().trim().replace(/[^\p{L}\p{N}\s_-]/gu, "").replace(/\s+/g, "-") || "section", count = ids.get(slug) ?? 0;
        ids.set(slug, count + 1); node.data = { ...node.data, hProperties: { id: count ? `${slug}-${count}` : slug } };
      }
      if ("children" in node) for (const child of node.children) visit(child);
    };
    visit(tree);
  };
}

// A link that can open. Anything else (a target dropped as unsafe, an email address) shows as plain text.
function WebLink({ href, children }: { href?: string; children?: ReactNode }) {
  const file = useContext(FileResources);
  const images = useContext(FileImages);
  if ((file || images) && href?.startsWith("#")) return <a href={href} onClick={(event) => { event.preventDefault(); const viewer = event.currentTarget.closest(".markdown"); try { viewer?.querySelector(`#${CSS.escape(decodeURIComponent(href.slice(1)))}`)?.scrollIntoView(); } catch { /* Invalid local anchors stay in this viewer. */ } }}>{children}</a>;
  if (!web(href)) return <>{children}</>;
  return (
    <LinkTarget value={href}>
      <a href={href} target="_blank" rel="noreferrer">
        {children}
      </a>
    </LinkTarget>
  );
}

// Images never load. A web image becomes a link to it; any other image shows its alt text. Inside another link it
// shows its alt text, or its address, or else (an address dropped as unsafe) the link's own address, so that link
// never ends up with nothing to read or click.
function MarkdownImage({ src, alt }: ComponentProps<"img">) {
  const file = useContext(FileResources);
  const images = useContext(FileImages);
  const link = useContext(LinkTarget);
  if (images && typeof src === "string") return link !== null ? <>{alt || src}</> : images(src, alt ?? "");
  const local = file && typeof src === "string" ? file(src) : null;
  // eslint-disable-next-line @next/next/no-img-element -- Scoped app handles must never go through an image optimizer.
  if (local && localImage(local)) return <img src={local} alt={alt ?? ""} loading="lazy" />;
  if (file) return <>{alt}</>;
  if (link !== null) return <>{alt || src || link}</>;
  return web(src) ? <WebLink href={src}>{alt || src}</WebLink> : <>{alt}</>;
}

// Bot replies are Markdown. Raw HTML never renders, unsafe link targets are dropped, and links open in the
// default browser (the desktop app sends every new window there).
export function MarkdownText({ text, fileResources, fileImages }: { text: string; fileResources?: (source: string) => string | null; fileImages?: (source: string, alt: string) => ReactNode }) {
  return (
    <FileResources value={fileResources ?? null}>
    <FileImages value={fileImages ?? null}>
    <div className="markdown">
      <Markdown remarkPlugins={fileResources || fileImages ? [remarkGfm, fileAnchors] : [remarkGfm]} components={{ a: WebLink, img: MarkdownImage }}>
        {text}
      </Markdown>
    </div>
    </FileImages>
    </FileResources>
  );
}
