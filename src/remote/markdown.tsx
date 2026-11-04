import { createElement, Fragment, memo, type ReactNode } from "react";
import MarkdownIt, { type Token } from "markdown-it";
import { IconTerminal2 } from "@tabler/icons-react";
import { imageUrl, messageUrl } from "./workspace-data";
import { FileLink, fileReference } from "./files";
import { CopyButton } from "./clipboard";

const markdown = new MarkdownIt({ html: false, linkify: true, breaks: true });
markdown.validateLink = url => !!messageUrl(url) || fileReference(url) || /^data:image\/(png|jpeg|gif|webp);base64,[a-z0-9+/=]+$/i.test(url);
const tags = new Set(["p", "h1", "h2", "h3", "h4", "h5", "h6", "ul", "ol", "li", "strong", "em", "s", "blockquote", "hr", "br", "table", "thead", "tbody", "tr", "th", "td"]);

export function CodeBlock({ text, language = "" }: { text: string; language?: string }) {
  return <div className="remote-code"><div className="remote-code-toolbar"><span><IconTerminal2 size={14} />{language || "代码"}</span><CopyButton text={text} label="复制代码" code /></div><pre tabIndex={0} aria-label="代码，可横向滚动"><code>{text}</code></pre></div>;
}

function tokensToReact(tokens: Token[]): ReactNode[] {
  let cursor = 0;
  function nodes(inLink = false): ReactNode[] {
    const result: ReactNode[] = [];
    while (cursor < tokens.length) {
      const index = cursor++, token = tokens[index];
      if (token.nesting === -1) break;
      const children = token.nesting === 1 ? nodes(inLink || token.type === "link_open") : undefined;
      if (token.hidden) { result.push(...children || []); continue; }
      const key = `${token.type}:${index}`;
      if (token.type === "inline") result.push(<Fragment key={key}>{tokensToReact(token.children || [])}</Fragment>);
      else if (token.type === "text") result.push(token.content);
      else if (token.type === "softbreak" || token.type === "hardbreak") result.push(<br key={key} />);
      else if (token.type === "code_inline") result.push(<code key={key}>{token.content}</code>);
      else if (token.type === "fence" || token.type === "code_block") result.push(<CodeBlock key={key} text={token.content} language={token.info.trim().split(/\s+/)[0]} />);
      else if (token.type === "link_open") {
        const reference = String(token.attrGet("href") || ""), url = messageUrl(reference);
        result.push(url ? <a key={key} href={url} title={String(token.attrGet("title") || "") || undefined} target="_blank" rel="noopener noreferrer">{children}</a>
          : fileReference(reference) ? <FileLink key={key} reference={reference} image={tokens[index + 1]?.type === "image"}>{children}</FileLink> : <span key={key}>{children}</span>);
      } else if (token.type === "image") {
        const reference = String(token.attrGet("src") || ""), url = messageUrl(reference), label = token.content || "图片";
        if (inLink) { result.push(label); continue; }
        result.push(/^data:image\//i.test(reference) && imageUrl(reference) ? <img key={key} src={reference} alt={label} loading="lazy" />
          : url ? <a key={key} href={url} target="_blank" rel="noopener noreferrer">{label}（图片）</a>
          : fileReference(reference) ? <FileLink key={key} reference={reference} image>{label}</FileLink> : <span key={key}>{label}（图片不可用）</span>);
      } else if (tags.has(token.tag)) {
        const alignment = /^text-align:(left|center|right)$/.exec(String(token.attrGet("style") || ""));
        const element = createElement(token.tag, { key, ...(alignment ? { className: `align-${alignment[1]}` } : {}), ...(token.tag === "ol" && token.attrGet("start") ? { start: Number(token.attrGet("start")) } : {}) }, children);
        result.push(token.tag === "table" ? <div key={key} className="remote-table" tabIndex={0} role="region" aria-label="表格，可横向滚动">{element}</div> : element);
      } else result.push(<span key={key}>不支持的内容，请在电脑端查看</span>);
    }
    return result;
  }
  return nodes();
}

export const RichText = memo(function RichText({ text }: { text: string }) {
  return <div className="remote-markdown">{tokensToReact(markdown.parse(text, {}))}</div>;
});
