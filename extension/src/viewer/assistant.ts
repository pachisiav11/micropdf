// The viewer's assistant: questions go through micropdf-bridge to the provider set up in micropdf
// for Windows, so API keys never reach the browser; chats are the desktop app's, by content hash.
// Answers come back as Markdown, parsed here into blocks the panel renders as elements (never as
// HTML), with [p. N] citations as page links.

import { type Connect, request } from "../bridge";

export interface Turn {
  role: "user" | "assistant";
  text: string;
  /** Under an answer: the pages it read and what it cost. */
  note?: string;
}

export interface Status {
  /** "Anthropic · claude-haiku-4-5". */
  model: string;
  /** What is missing before the assistant can answer; empty when nothing is. */
  notice: string;
  /** Today's usage with the provider. */
  usage: string;
}

export interface Question {
  hash: string;
  prompt: string;
  /** Each page's text. */
  pages: string[];
  /** The page being read, from 0, kept first when only some pages fit. */
  focus?: number;
  /** The question is about that page alone. */
  pageOnly?: boolean;
}

export const status = (connect?: Connect): Promise<Status> => request<Status>({ type: "assistant" }, connect);

export const loadChat = async (hash: string, connect?: Connect): Promise<Turn[]> =>
  (await request<{ turns: Turn[] }>({ type: "chat", hash }, connect)).turns;

export const newChat = async (hash: string, connect?: Connect): Promise<void> => {
  await request({ type: "new-chat", hash }, connect);
};

export const ask = (q: Question, connect?: Connect): Promise<{ turn: Turn; usage: string }> =>
  request<{ turn: Turn; usage: string }>({ type: "ask", ...q }, connect);

export interface Inline {
  text: string;
  strong?: boolean;
  em?: boolean;
  code?: boolean;
  /** A cited page, from 1. */
  page?: number;
  href?: string;
}

export type Block =
  | { kind: "p" | "h"; inlines: Inline[] }
  | { kind: "pre"; text: string }
  | { kind: "ul" | "ol"; items: Inline[][] };

// Bold, italic, code, a citation such as [p. 12] (not already a link), an http(s) link.
const INLINE = /\*\*(.+?)\*\*|\*(?!\s)([^*]+?)\*|`([^`]+)`|\[(pp?\.\s*(\d+)[^\]]{0,16})\](?!\()|\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g;

export function inlines(text: string): Inline[] {
  const out: Inline[] = [];
  let last = 0;
  for (const m of text.matchAll(INLINE)) {
    if (m.index > last) out.push({ text: text.slice(last, m.index) });
    if (m[1] !== undefined) out.push(...inlines(m[1]).map((i) => ({ ...i, strong: true })));
    else if (m[2] !== undefined) out.push(...inlines(m[2]).map((i) => ({ ...i, em: true })));
    else if (m[3] !== undefined) out.push({ text: m[3], code: true });
    else if (m[4] !== undefined) {
      const page = Number(m[5]);
      out.push(page > 0 ? { text: m[4], page } : { text: m[0] });
    } else out.push({ text: m[6], href: m[7] });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}

/** The answer's blocks: paragraphs, headings, lists and code; quotes and table rows read as
 * paragraphs, and rules and table borders go. */
export function blocks(markdown: string): Block[] {
  const out: Block[] = [];
  let paragraph: string[] = [];
  let list: { kind: "ul" | "ol"; items: string[] } | null = null;
  let code: string[] | null = null;
  const flush = () => {
    if (paragraph.length) out.push({ kind: "p", inlines: inlines(paragraph.join(" ")) });
    if (list) out.push({ kind: list.kind, items: list.items.map(inlines) });
    paragraph = [];
    list = null;
  };
  for (const line of markdown.replace(/\r\n?/g, "\n").split("\n")) {
    const fence = /^\s*(```|~~~)/.test(line);
    if (code) {
      if (fence) {
        out.push({ kind: "pre", text: code.join("\n") });
        code = null;
      } else code.push(line);
      continue;
    }
    const t = line.trim();
    if (fence || !t || /^([-*_=])(\s*\1){2,}$/.test(t) || /^\|[\s:|-]*-[\s:|-]*$/.test(t)) {
      flush();
      if (fence) code = [];
      continue;
    }
    const heading = /^#{1,6}\s+(.*?)[\s#]*$/.exec(t);
    const item = /^(?:([-*+])|\d+[.)])\s+(.*)$/.exec(t);
    if (heading) {
      flush();
      out.push({ kind: "h", inlines: inlines(heading[1]) });
    } else if (t.startsWith("|")) {
      flush();
      const cells = t.replace(/^\||\|$/g, "").split("|");
      out.push({ kind: "p", inlines: inlines(cells.map((c) => c.trim()).join(" · ")) });
    } else if (item) {
      const kind = item[1] ? "ul" : "ol";
      if (paragraph.length || (list && list.kind !== kind)) flush();
      list ??= { kind, items: [] };
      list.items.push(item[2]);
    } else if (list && /^\s/.test(line)) {
      list.items[list.items.length - 1] += ` ${t}`;
    } else {
      if (list) flush();
      paragraph.push(t.replace(/^(>\s?)+/, ""));
    }
  }
  if (code) out.push({ kind: "pre", text: code.join("\n") });
  flush();
  return out;
}
