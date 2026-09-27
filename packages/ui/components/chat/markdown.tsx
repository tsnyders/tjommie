import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

// Bot replies are Markdown. Raw HTML never renders, unsafe link targets are dropped, and links open in the
// default browser (the desktop app sends every new window there).
export function MarkdownText({ text }: { text: string }) {
  return (
    <div className="markdown">
      <Markdown
        remarkPlugins={[remarkGfm]}
        components={{
          a: ({ href, children }) => (
            <a href={href} target="_blank" rel="noreferrer">
              {children}
            </a>
          ),
        }}
      >
        {text}
      </Markdown>
    </div>
  );
}
