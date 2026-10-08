// The mupdf.js worker: holds the document and draws pages off the page's thread.

import type * as Mupdf from "mupdf";
import type { Rect } from "./layout";
import type {
  AnnotInfo,
  Call,
  Drawn,
  Edited,
  FieldInfo,
  Hit,
  NewAnnot,
  Opened,
  OutlineNode,
  PageLink,
  Point,
  Reply,
  Request,
  Rgb,
  Saved,
  Target,
  TextLine,
} from "./pdf";
import { contentHash } from "./hash";
import { type ReadingMode, toRgba } from "./recolor";

// mupdf.js loads its WebAssembly with a top-level await. Imported statically, it would hold up
// this module, and the page's first messages would arrive before onmessage is set and be lost.
const loading = import("mupdf");
let mupdf: typeof Mupdf;
let doc: Mupdf.Document | null = null;
/** The password that opened the document, to open it again after a save. */
let password = "";
/** The opened file's content hash. */
let hash = "";
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
  if (!unlocked && d.needsPassword()) return { needsPassword: true, pages: [], title: "", hash };
  const pages = [];
  for (let i = 0; i < d.countPages(); i++) {
    const page = d.loadPage(i);
    const [x0, y0, x1, y1] = page.getBounds();
    pages.push({ width: x1 - x0, height: y1 - y0 });
    page.destroy();
  }
  return { needsPassword: false, pages, title: d.getMetaData("info:Title") ?? "", hash };
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

function pageTexts(): string[] {
  const d = loaded();
  const out = [];
  for (let i = 0; i < d.countPages(); i++) {
    const page = d.loadPage(i);
    const stext = page.toStructuredText("");
    out.push(stext.asText());
    stext.destroy();
    page.destroy();
  }
  return out;
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

function pdfDoc(): Mupdf.PDFDocument {
  const d = loaded().asPDF();
  if (!d) throw new Error("only PDF documents can be changed");
  return d;
}

function pdfPage(index: number): Mupdf.PDFPage {
  return pdfDoc().loadPage(index) as Mupdf.PDFPage;
}

const idOf = (o: { getObject(): Mupdf.PDFObject }) => o.getObject().asIndirect();
const rectOf = ([x0, y0, x1, y1]: Mupdf.Rect): Rect => ({ x0, y0, x1, y1 });
const box = (r: Rect): Mupdf.Rect => [r.x0, r.y0, r.x1, r.y1];

/** Drops the drawn pages an edit changed, all of them when `pages` is null. */
function forget(pages: number[] | null): void {
  for (const [i, list] of lists) {
    if (pages && !pages.includes(i)) continue;
    list.destroy();
    lists.delete(i);
  }
}

/** Runs `change` as one step of the undo history. */
function edit<T extends object>(name: string, pages: number[] | null, change: () => T): T & Edited {
  const d = pdfDoc();
  d.beginOperation(name);
  let result: T;
  try {
    result = change();
  } catch (e) {
    d.abandonOperation();
    throw e;
  }
  d.endOperation();
  forget(pages);
  return { ...result, pages, canUndo: d.canUndo(), canRedo: d.canRedo() };
}

function comments(page: Mupdf.PDFPage): Mupdf.PDFAnnotation[] {
  return page.getAnnotations().filter((a) => !["Popup", "Link", "Widget"].includes(a.getType()));
}

const locked = (a: Mupdf.PDFAnnotation) => (a.getFlags() & mupdf.PDFAnnotation.IS_LOCKED) !== 0;

function describeAnnot(a: Mupdf.PDFAnnotation): AnnotInfo {
  return {
    id: idOf(a),
    type: a.getType(),
    rect: rectOf(a.getBounds()),
    contents: a.getContents(),
    author: a.getAuthor(),
    locked: locked(a),
  };
}

function annotAt(index: number, [x, y]: Point): AnnotInfo | null {
  const page = pdfPage(index);
  // Thin lines and ink are hard to hit exactly; allow a couple of points around them.
  const slack = 2;
  const hit = comments(page)
    .reverse()
    .find((a) => {
      const [x0, y0, x1, y1] = a.getBounds();
      return x >= x0 - slack && x <= x1 + slack && y >= y0 - slack && y <= y1 + slack;
    });
  const info = hit ? describeAnnot(hit) : null;
  page.destroy();
  return info;
}

function findAnnot(page: Mupdf.PDFPage, id: number): Mupdf.PDFAnnotation {
  const a = comments(page).find((c) => idOf(c) === id);
  if (!a) throw new Error("the comment is gone");
  if (locked(a)) throw new Error("the comment is locked");
  return a;
}

const MARKUP = { highlight: "Highlight", underline: "Underline", strikeout: "StrikeOut" } as const;

function addAnnot(index: number, n: NewAnnot, author: string): { id: number } {
  const page = pdfPage(index);
  let a: Mupdf.PDFAnnotation;
  switch (n.kind) {
    case "note": {
      const [x, y] = n.at;
      a = page.createAnnotation("Text");
      a.setRect([x, y, x + 20, y + 20]);
      a.setIcon("Comment");
      a.setContents(n.text);
      a.setColor(n.color);
      break;
    }
    case "text":
      a = page.createAnnotation("FreeText");
      a.setRect(box(n.rect));
      a.setContents(n.text);
      // A text box's /C is its fill; the text, border and callout take the DA colour.
      a.setDefaultAppearance("Helv", n.size, n.color);
      break;
    case "ink":
      a = page.createAnnotation("Ink");
      a.setInkList(n.strokes);
      a.setColor(n.color);
      a.setBorderWidth(n.width);
      break;
    case "square":
    case "circle":
      a = page.createAnnotation(n.kind === "square" ? "Square" : "Circle");
      a.setRect(box(n.rect));
      a.setColor(n.color);
      a.setBorderWidth(n.width);
      break;
    case "line":
      a = page.createAnnotation("Line");
      a.setLine(n.from, n.to);
      if (n.arrow) a.setLineEndingStyles("None", "OpenArrow");
      a.setColor(n.color);
      a.setBorderWidth(n.width);
      break;
    default: {
      const stext = page.toStructuredText("");
      const quads = n.spans.flatMap(([from, to]) => stext.highlight(from, to));
      stext.destroy();
      if (!quads.length) throw new Error("there is no text there");
      a = page.createAnnotation(MARKUP[n.kind]);
      a.setQuadPoints(quads);
      a.setColor(n.color);
    }
  }
  if (author) a.setAuthor(author);
  a.update();
  const id = idOf(a);
  page.destroy();
  return { id };
}

function editAnnot(index: number, id: number, contents?: string, color?: Rgb): object {
  const page = pdfPage(index);
  const a = findAnnot(page, id);
  if (contents !== undefined) a.setContents(contents);
  if (color && a.getType() === "FreeText") {
    const da = a.getDefaultAppearance();
    a.setDefaultAppearance(da.font, da.size, color);
  } else if (color) {
    a.setColor(color);
  }
  a.update();
  page.destroy();
  return {};
}

function deleteAnnot(index: number, id: number): object {
  const page = pdfPage(index);
  page.deleteAnnotation(findAnnot(page, id));
  page.destroy();
  return {};
}

function fields(index: number): FieldInfo[] {
  const page = pdfPage(index);
  const out = page.getWidgets().map((w): FieldInfo => {
    const type = w.getFieldType() as FieldInfo["type"];
    const state = w.getObject().get("AS");
    return {
      id: idOf(w),
      type,
      name: w.getName(),
      rect: rectOf(w.getBounds()),
      value: w.getValue(),
      checked: state.isName() && state.asName() !== "Off",
      options: type === "combobox" || type === "listbox" ? w.getOptions() : [],
      readOnly: w.isReadOnly(),
      multiline: w.isMultiline(),
      maxLen: w.getMaxLen(),
    };
  });
  page.destroy();
  return out;
}

/** Drops any XFA packet, which XFA readers would show instead of the new values. */
function dropXfa(d: Mupdf.PDFDocument): void {
  const form = d.getTrailer().get("Root", "AcroForm");
  if (form.isDictionary() && !form.get("XFA").isNull()) form.delete("XFA");
}

const SIMPLE_CALCULATE = /AFSimple_Calculate\s*\(\s*["'](\w+)["']\s*,\s*(?:new\s+Array\s*\(([^)]*)\)|\[([^\]]*)\])/;

const REDUCE: Record<string, (v: number[]) => number> = {
  SUM: (v) => v.reduce((a, b) => a + b, 0),
  PRD: (v) => v.reduce((a, b) => a * b, 1),
  AVG: (v) => (v.length ? v.reduce((a, b) => a + b, 0) / v.length : 0),
  MIN: (v) => (v.length ? Math.min(...v) : 0),
  MAX: (v) => (v.length ? Math.max(...v) : 0),
};

/** mupdf.js has no JavaScript engine, so form scripts do not run. This does the calculation most
 * forms use, AFSimple_Calculate (a sum, product, average, minimum or maximum of other fields),
 * for each field in the form's calculation order. */
function calculate(d: Mupdf.PDFDocument): void {
  const order = d.getTrailer().get("Root", "AcroForm", "CO");
  if (!order.isArray()) return;
  const widgets: Mupdf.PDFWidget[] = [];
  for (let i = 0; i < d.countPages(); i++) widgets.push(...pdfPage(i).getWidgets());
  const byName = new Map(widgets.map((w) => [w.getName(), w]));
  for (let k = 0; k < order.length; k++) {
    const field = order.get(k);
    const js = field.get("AA", "C", "JS");
    const script = js.isStream() ? js.readStream().asString() : js.isString() ? js.asString() : "";
    const match = SIMPLE_CALCULATE.exec(script);
    const reduce = match && REDUCE[match[1]];
    const target = widgets.find((w) => {
      // The calculated field's own widget, or one of its kids.
      for (let o = w.getObject(); o.isDictionary(); o = o.get("Parent")) {
        if (o.asIndirect() === field.asIndirect()) return true;
      }
      return false;
    });
    if (!reduce || !target) continue;
    const values = (match[2] ?? match[3])
      .split(",")
      .map((name) => byName.get(name.trim().replace(/^["']|["']$/g, ""))?.getValue() ?? "")
      .map((v) => Number.parseFloat(v.replace(/,/g, "")) || 0);
    target.setTextValue(String(Number(reduce(values).toFixed(10))));
    target.update();
  }
}

function changeField(index: number, id: number, change: (w: Mupdf.PDFWidget) => void): object {
  const d = pdfDoc();
  dropXfa(d);
  const page = pdfPage(index);
  const w = page.getWidgets().find((x) => idOf(x) === id);
  if (!w) throw new Error("the field is gone");
  if (w.isReadOnly()) throw new Error("the field is read-only");
  change(w);
  w.update();
  calculate(d);
  page.destroy();
  return {};
}

function history(step: "undo" | "redo"): Edited {
  const d = pdfDoc();
  const can = step === "undo" ? d.canUndo() : d.canRedo();
  if (can) {
    d[step]();
    forget(null);
  }
  return { pages: can ? null : [], canUndo: d.canUndo(), canRedo: d.canRedo() };
}

function open(data: Uint8Array | ArrayBuffer): Mupdf.Document {
  forget(null);
  doc?.destroy();
  doc = mupdf.Document.openDocument(data, "application/pdf");
  doc.asPDF()?.enableJournal();
  return doc;
}

function bytesOf(buffer: Mupdf.Buffer): Uint8Array<ArrayBuffer> {
  const bytes = new Uint8Array(buffer.asUint8Array());
  buffer.destroy();
  return bytes;
}

/** The document's bytes with every change, appended to the original. A changed document's undo
 * history ends here: mupdf takes a saved update as part of the file it opened, so a second
 * incremental save would point at offsets the original bytes lack; the document is opened again
 * from what was saved. An unchanged one saves as its original bytes. */
function save(): Saved {
  const d = pdfDoc();
  // A repaired file cannot take an incremental update; it is written whole.
  const bytes = bytesOf(d.saveToBuffer(d.wasRepaired() ? "" : "incremental"));
  if (d.hasUnsavedChanges()) {
    const reopened = open(bytes);
    if (reopened.needsPassword()) reopened.authenticatePassword(password);
  }
  return { bytes, canUndo: pdfDoc().canUndo(), canRedo: pdfDoc().canRedo() };
}

/** The document's bytes with every change, leaving it and its history as they are: a whole new
 * file when changed, which unlike an incremental save does not move mupdf's idea of the file. */
function copy(): Uint8Array<ArrayBuffer> {
  const d = pdfDoc();
  return bytesOf(d.saveToBuffer(d.hasUnsavedChanges() || d.wasRepaired() ? "" : "incremental"));
}

function handle(request: Request): unknown {
  switch (request.method) {
    case "open":
      password = "";
      hash = contentHash(new Uint8Array(request.data));
      return describe(open(request.data));
    case "unlock":
      if (!loaded().authenticatePassword(request.password)) return null;
      password = request.password;
      return describe(loaded(), true);
    case "outline":
      return outline(loaded().loadOutline() ?? []);
    case "render":
      return render(request.page, request.scale, request.mode, request.clip);
    case "text":
      return text(request.page);
    case "pageTexts":
      return pageTexts();
    case "links":
      return links(request.page);
    case "search":
      return search(request.page, request.needle);
    case "annotAt":
      return annotAt(request.page, request.at);
    case "addAnnot":
      return edit("Add comment", [request.page], () =>
        addAnnot(request.page, request.annot, request.author),
      );
    case "editAnnot":
      return edit("Change comment", [request.page], () =>
        editAnnot(request.page, request.id, request.contents, request.color),
      );
    case "deleteAnnot":
      return edit("Delete comment", [request.page], () => deleteAnnot(request.page, request.id));
    case "fields":
      return fields(request.page);
    // Calculations can change fields on any page.
    case "setField":
      return edit("Fill in field", null, () =>
        changeField(request.page, request.id, (w) =>
          w.isChoice() ? w.setChoiceValue(request.value) : w.setTextValue(request.value),
        ),
      );
    case "toggleField":
      return edit("Fill in field", null, () => changeField(request.page, request.id, (w) => w.toggle()));
    case "undo":
    case "redo":
      return history(request.method);
    case "save":
      return save();
    case "copy":
      return copy();
  }
}

self.onmessage = async (e: MessageEvent<Call>) => {
  const { id, request } = e.data;
  let reply: Reply;
  try {
    mupdf = await loading;
    reply = { id, ok: true, result: handle(request) };
  } catch (err) {
    reply = { id, ok: false, error: err instanceof Error ? err.message : String(err) };
  }
  const result = reply.ok ? reply.result : null;
  // Pixels and file bytes move to the page instead of being copied.
  const moved = result instanceof Uint8Array ? result : ((result as Partial<Drawn & Saved>)?.pixels ?? (result as Partial<Saved>)?.bytes);
  const transfer = moved ? [moved.buffer] : [];
  (self as unknown as Worker).postMessage(reply, transfer);
};
