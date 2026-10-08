// The mupdf.js worker: holds the document and draws pages off the page's thread.

import type * as Mupdf from "mupdf";
import type { Drawn, Opened, Reply, Request, TextLine } from "./pdf";

// mupdf.js loads its WebAssembly with a top-level await. Imported statically, it would hold up
// this module, and the page's first messages would arrive before onmessage is set and be lost.
const loading = import("mupdf");
let mupdf: typeof Mupdf;
let doc: Mupdf.Document | null = null;

function loaded(): Mupdf.Document {
  if (!doc) throw new Error("no document is open");
  return doc;
}

function describe(d: Mupdf.Document): Opened {
  if (d.needsPassword()) return { needsPassword: true, pages: [], title: "" };
  const pages = [];
  for (let i = 0; i < d.countPages(); i++) {
    const page = d.loadPage(i);
    const [x0, y0, x1, y1] = page.getBounds();
    pages.push({ width: x1 - x0, height: y1 - y0 });
    page.destroy();
  }
  return { needsPassword: false, pages, title: d.getMetaData("info:Title") ?? "" };
}

function render(index: number, scale: number): Drawn {
  const page = loaded().loadPage(index);
  const pix = page.toPixmap(mupdf.Matrix.scale(scale, scale), mupdf.ColorSpace.DeviceRGB, false, true);
  const [width, height, stride] = [pix.getWidth(), pix.getHeight(), pix.getStride()];
  const rgb = pix.getPixels();
  const pixels = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y++) {
    let s = y * stride;
    let d = y * width * 4;
    for (let x = 0; x < width; x++, s += 3, d += 4) {
      pixels[d] = rgb[s];
      pixels[d + 1] = rgb[s + 1];
      pixels[d + 2] = rgb[s + 2];
      pixels[d + 3] = 255;
    }
  }
  pix.destroy();
  page.destroy();
  return { width, height, pixels };
}

interface JsonLine {
  wmode: number;
  bbox: { x: number; y: number; w: number; h: number };
  font: { size: number };
  text: string;
}

function text(index: number): TextLine[] {
  const page = loaded().loadPage(index);
  const stext = page.toStructuredText("preserve-whitespace");
  const json = JSON.parse(stext.asJSON()) as { blocks: { type: string; lines?: JsonLine[] }[] };
  stext.destroy();
  page.destroy();
  return json.blocks
    .flatMap((b) => (b.type === "text" ? (b.lines ?? []) : []))
    .filter((l) => l.wmode === 0 && l.text.trim() !== "")
    .map((l) => ({ ...l.bbox, size: l.font.size, text: l.text }));
}

function handle(request: Request): unknown {
  switch (request.method) {
    case "open":
      doc = mupdf.Document.openDocument(new Uint8Array(request.data), "application/pdf");
      return describe(doc);
    case "unlock":
      return loaded().authenticatePassword(request.password) ? describe(loaded()) : null;
    case "render":
      return render(request.page, request.scale);
    case "text":
      return text(request.page);
  }
}

self.onmessage = async (e: MessageEvent<Request & { id: number }>) => {
  const { id } = e.data;
  let reply: Reply;
  try {
    mupdf = await loading;
    reply = { id, ok: true, result: handle(e.data) };
  } catch (err) {
    reply = { id, ok: false, error: err instanceof Error ? err.message : String(err) };
  }
  const result = reply.ok ? (reply.result as Partial<Drawn> | null) : null;
  const transfer = result?.pixels ? [result.pixels.buffer] : [];
  (self as unknown as Worker).postMessage(reply, transfer);
};
