import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import rehypeKatex from "rehype-katex";
import "katex/dist/katex.min.css";
import "../styles/memo-content.css";

export function formatMemoTime(value: string): string {
  if (!value) return "时间未知";
  const date = /^\d+$/.test(value)
    ? new Date(Number(value) * (Number(value) < 1e12 ? 1000 : 1))
    : new Date(value);
  if (!Number.isFinite(date.getTime())) return "时间未知";
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false
  }).format(date);
}

export function MemoContent({ content }: { content: string }) {
  return <div className="memo-content">
    <Markdown skipHtml remarkPlugins={[remarkGfm, remarkMath]}
      rehypePlugins={[[rehypeKatex, { trust: false, strict: "ignore" }]]}
      components={{
        a: ({ children, href }) => <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>,
        // Remote attachments need the authenticated backend; do not silently send
        // private memo attachment URLs or tokens from the renderer to other hosts.
        img: ({ alt }) => <span className="memo-attachment">[图片{alt ? `：${alt}` : ""}]</span>
      }}>
      {content}
    </Markdown>
  </div>;
}
