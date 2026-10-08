import { describe, expect, it } from "vitest";
import { CHUNK, type PortLike, openLocal, sendPdf } from "./bridge";

/** A bridge that answers like micropdf-bridge, or fails at `failOn`. */
function fakeBridge(failOn?: string) {
  const sent: Record<string, unknown>[] = [];
  let reply: (m: { type: string; path?: string; message?: string }) => void = () => {};
  let disconnected = false;
  const port: PortLike = {
    postMessage(message) {
      const m = message as Record<string, unknown>;
      sent.push(m);
      queueMicrotask(() => {
        if (m.type === failOn) return reply({ type: "error", message: "nope" });
        if (m.type === "begin") reply({ type: "ready" });
        if (m.type === "chunk") reply({ type: "chunk" });
        if (m.type === "end" || m.type === "open") reply({ type: "done", path: "C:\\in\\a.pdf" });
      });
    },
    onMessage: { addListener: (l) => (reply = l) },
    onDisconnect: { addListener: () => {} },
    disconnect: () => (disconnected = true),
  };
  return { port, sent, closed: () => disconnected };
}

describe("sendPdf", () => {
  it("sends the file in acknowledged chunks", async () => {
    const bridge = fakeBridge();
    const bytes = new Uint8Array(CHUNK + 10).fill(65);
    const path = await sendPdf(bytes, { name: "a.pdf" }, () => bridge.port);
    expect(path).toBe("C:\\in\\a.pdf");
    expect(bridge.sent.map((m) => m.type)).toEqual(["begin", "chunk", "chunk", "end"]);
    expect(bridge.sent[0]).toEqual({ type: "begin", name: "a.pdf" });
    const data = bridge.sent.slice(1, 3).map((m) => atob(m.data as string).length);
    expect(data).toEqual([CHUNK, 10]);
    expect(bridge.closed()).toBe(true);
  });

  it("stops at the bridge's first error", async () => {
    const bridge = fakeBridge("begin");
    await expect(sendPdf(new Uint8Array(4), { target: "C:\\x.pdf" }, () => bridge.port)).rejects.toThrow("nope");
    expect(bridge.sent).toHaveLength(1);
    expect(bridge.closed()).toBe(true);
  });
});

it("opens a local file as it is", async () => {
  const bridge = fakeBridge();
  expect(await openLocal("C:\\x.pdf", () => bridge.port)).toBe("C:\\in\\a.pdf");
  expect(bridge.sent).toEqual([{ type: "open", path: "C:\\x.pdf" }]);
});
