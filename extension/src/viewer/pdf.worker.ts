// The mupdf.js worker: holds the document and draws pages off the page's thread.

import type * as Mupdf from "mupdf";
import type { Rect } from "./layout";
import type { Drawn, Hit, Opened, OutlineNode, PageLink, Reply, Request, Target, TextLine } from "./pdf";
import { type ReadingMode, toRgba } from "./recolor";

// mupdf.js loads its WebAssembly with a top-level await. Imported statically, it would hold up
// this module, and the page's first messages would arrive before onmessage is set and be lost.
const loading = import("mupdf");
let mupdf: typeof Mupdf;
let doc: Mupdf.Document | null = null;
/** Recently drawn pages, most recent last: tiles of the same page draw from one list. */
const lists = new Map<number, Mupdf.DisplayList>();
const LISTS = 8;

function loaded(): Mupdf.Document {
  if (!doc) throw new Error("no document is open");
  return doc;
}

/** The document's pages; `unlocked` once a password was accepted, since needsPassword() only
 * tries the empty one and stays true. */
function describe(d: Mupdf.Document, unlocked = false): Opened {
  if (!unlocked && d.needsPassword()) return { needsPassword: true, pages: [], title: "" };
  const pages = [];
  for (let i = 0; i < d.countPages(); i++) {
    const page = d.loadPage(i);
    const [x0, y0, x1, y1] = page.getBounds();
    pages.push({ width: x1 - x0, height: y1 - y0 });
    page.destroy();
  }
  return { needsPassword: false, pages, title: d.getMetaData("info:Title") ?? "" };
}

/** Where `uri` leads: a page and height in this document, or the URI itself. Of other
 * schemes, only web and mail links are kept; a PDF's javascript: or file: links go nowhere. */
function target(uri: string | undefined, page?: number): Target {
  if (uri && /^\w[\w+-.]*:/.test(uri)) return /^(https?|mailto):/i.test(uri) ? { uri } : {};
  if (!uri) return page === undefined ? {} : { page };
  try {
    const dest = loaded().resolveLinkDestination(uri);
    const top = ["XYZ", "FitH", "FitBH", "FitR"].includes(dest.type) && Number.isFinite(dest.y);
    return dest.page < 0 ? {} : top ? { page: dest.page, top: dest.y } : { page: dest.page };
  } catch {
    return page === undefined ? {} : { page };
  }
}

interface MupdfOutline {
  title?: string;
  uri?: string;
  open: boolean;
  page?: number;
  down?: MupdfOutline[];
}

function outline(items: MupdfOutline[]): OutlineNode[] {
  return items.map((item) => ({
    title: item.title ?? "",
    open: item.open,
    ...target(item.uri, item.page),
    children: outline(item.down ?? []),
  }));
}

function displayList(index: number): Mupdf.DisplayList {
  let list = lists.get(index);
  if (list) {
    lists.delete(index);
  } else {
    const page = loaded().loadPage(index);
    list = page.toDisplayList(true);
    page.destroy();
    if (lists.size >= LISTS) {
      const [oldest, dropped] = lists.entries().next().value!;
      lists.delete(oldest);
      dropped.destroy();
    }
  }
  lists.set(index, list);
  return list;
}

/** Draws the page, or only `clip` (page space) of it, at `scale` device pixels per point. */
function render(index: number, scale: number, mode: ReadingMode, clip?: Rect): Drawn {
  const list = displayList(index);
  const matrix = mupdf.Matrix.scale(scale, scale);
  let pix: Mupdf.Pixmap;
  let [x, y] = [0, 0];
  if (clip) {
    [x, y] = [Math.floor(clip.x0 * scale), Math.floor(clip.y0 * scale)];
    const bbox: Mupdf.Rect = [x, y, Math.ceil(clip.x1 * scale), Math.ceil(clip.y1 * scale)];
    pix = new mupdf.Pixmap(mupdf.ColorSpace.DeviceRGB, bbox, false);
    pix.clear(255);
    const device = new mupdf.DrawDevice(mupdf.Matrix.identity, pix);
    list.run(device, matrix);
    device.close();
    device.destroy();
  } else {
    pix = list.toPixmap(matrix, mupdf.ColorSpace.DeviceRGB, false);
  }
  const [width, height] = [pix.getWidth(), pix.getHeight()];
  const pixels = toRgba(pix.getPixels(), width, height, pix.getStride(), mode);
  pix.destroy();
  return { x, y, width, height, pixels };
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

function links(index: number): PageLink[] {
  const page = loaded().loadPage(index);
  const out = page.getLinks().map((link) => {
    const [x0, y0, x1, y1] = link.getBounds();
    const result = { rect: { x0, y0, x1, y1 }, ...target(link.getURI()) };
    link.destroy();
    return result;
  });
  page.destroy();
  return out.filter((l) => l.uri !== undefined || l.page !== undefined);
}

function bounds(quad: number[]): Rect {
  const xs = [quad[0], quad[2], quad[4], quad[6]];
  const ys = [quad[1], quad[3], quad[5], quad[7]];
  return { x0: Math.min(...xs), y0: Math.min(...ys), x1: Math.max(...xs), y1: Math.max(...ys) };
}

function search(index: number, needle: string): Hit[] {
  const page = loaded().loadPage(index);
  const hits = page.search(needle, "ignore-case").map((quads) => quads.map(bounds));
  page.destroy();
  return hits;
}

function handle(request: Request): unknown {
  switch (request.method) {
    case "open":
      for (const list of lists.values()) list.destroy();
      lists.clear();
      doc = mupdf.Document.openDocument(new Uint8Array(request.data), "application/pdf");
      return describe(doc);
    case "unlock":
      return loaded().authenticatePassword(request.password) ? describe(loaded(), true) : null;
    case "outline":
      return outline(loaded().loadOutline() ?? []);
    case "render":
      return render(request.page, request.scale, request.mode, request.clip);
    case "text":
      return text(request.page);
    case "links":
      return links(request.page);
    case "search":
      return search(request.page, request.needle);
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
