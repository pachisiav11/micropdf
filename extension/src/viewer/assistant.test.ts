import { describe, expect, it } from "vitest";
import type { PortLike } from "../bridge";
import { ask, blocks, inlines, loadChat, status } from "./assistant";
import { contentHash } from "./hash";

describe("contentHash", () => {
  it("matches mp-ai's FNV-1a", () => {
    const bytes = (s: string) => new TextEncoder().encode(s);
    expect(contentHash(bytes(""))).toBe("cbf29ce484222325");
    expect(contentHash(bytes("a"))).toBe("af63dc4c8601ec8c");
    expect(contentHash(bytes("foobar"))).toBe("85944171f73967e8");
    expect(contentHash(new Uint8Array(1000).fill(255))).toMatch(/^[0-9a-f]{16}$/);
  });
});

describe("answers", () => {
  it("turn citations into page links and keep the rest as text", () => {
    expect(inlines("Costs rose [p. 12], see [pp. 3–4] and **[p. 5]**.")).toEqual([
      { text: "Costs rose " },
      { text: "p. 12", page: 12 },
      { text: ", see " },
      { text: "pp. 3–4", page: 3 },
      { text: " and " },
      { text: "p. 5", page: 5, strong: true },
      { text: "." },
    ]);
    expect(inlines("[note] [p. 0] [site](https://example.com) `a*b` *it*")).toEqual([
      { text: "[note] " },
      { text: "[p. 0]" },
      { text: " " },
      { text: "site", href: "https://example.com" },
      { text: " " },
      { text: "a*b", code: true },
      { text: " " },
      { text: "it", em: true },
    ]);
  });

  it("split into paragraphs, headings, lists and code", () => {
    const md = "# Summary\n\nThe report\nfinds:\n\n- costs rose\n  sharply\n- 3 < 4\n\n1. one\n2. two\n---\n> quoted\n\n| a | b |\n|---|---|\n\n```\nx = 1\n```";
    expect(blocks(md)).toEqual([
      { kind: "h", inlines: [{ text: "Summary" }] },
      { kind: "p", inlines: [{ text: "The report finds:" }] },
      { kind: "ul", items: [[{ text: "costs rose sharply" }], [{ text: "3 < 4" }]] },
      { kind: "ol", items: [[{ text: "one" }], [{ text: "two" }]] },
      { kind: "p", inlines: [{ text: "quoted" }] },
      { kind: "p", inlines: [{ text: "a · b" }] },
      { kind: "pre", text: "x = 1" },
    ]);
  });
});

describe("the assistant over the bridge", () => {
  /** A stand-in bridge that answers each message with `reply(message)`. */
  function bridge(reply: (m: Record<string, unknown>) => object) {
    const sent: Record<string, unknown>[] = [];
    const connect = (): PortLike => {
      let listener: (m: never) => void = () => {};
      return {
        postMessage: (m) => {
          sent.push(m as Record<string, unknown>);
          queueMicrotask(() => listener(reply(m as Record<string, unknown>) as never));
        },
        onMessage: { addListener: (l) => (listener = l as never) },
        onDisconnect: { addListener: () => {} },
        disconnect: () => {},
      };
    };
    return { sent, connect };
  }

  it("asks with the page text and gets the answer", async () => {
    const { sent, connect } = bridge((m) =>
      m.type === "ask"
        ? { type: "answer", turn: { role: "assistant", text: "Hi [p. 1]" }, usage: "today 9 in · 2 out" }
        : m.type === "chat"
          ? { type: "chat", turns: [{ role: "user", text: "q" }] }
          : { type: "assistant", model: "Anthropic · m", notice: "", usage: "" },
    );
    const q = { hash: "00", prompt: "What?", pages: ["Hello"], focus: 0 };
    expect((await ask(q, connect)).turn.text).toBe("Hi [p. 1]");
    expect(sent[0]).toEqual({ type: "ask", ...q });
    expect(await loadChat("00", connect)).toEqual([{ role: "user", text: "q" }]);
    expect((await status(connect)).model).toBe("Anthropic · m");
  });

  it("passes on what went wrong", async () => {
    const { connect } = bridge(() => ({ type: "error", message: "No model set" }));
    await expect(ask({ hash: "00", prompt: "?", pages: [] }, connect)).rejects.toThrow("No model set");
  });
});
