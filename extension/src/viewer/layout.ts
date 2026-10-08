// Page geometry for the document view, in CSS pixels; a port of the desktop's app/src/layout.rs.
//
// "Page space" is MuPDF's: PDF points, origin at the unrotated page's top left, y down.
// "Document space" is the scrolling column: CSS pixels, origin at its top left.

import type { PageSize } from "./pdf";

/** CSS pixels per PDF point at 100%: a point is 1/72 inch, a CSS pixel 1/96. */
export const PX_PER_POINT = 96 / 72;
/** Space around the document and between pages, in CSS pixels. */
export const MARGIN = 24;
export const GAP = 16;
/** Arrow-key scroll step, in CSS pixels. */
export const LINE = 60;

/** Zoom steps as fractions of 100%, as on the desktop. */
export const ZOOMS = [
  0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4, 5, 6.4, 8, 12,
];
const MIN_SCALE = 0.05;
const MAX_SCALE = 16;

export type PageMode = "single" | "continuous" | "two-up" | "book";
/** Fit width, fit page, or CSS pixels per point. */
export type Zoom = "fit-width" | "fit-page" | number;

export interface Frame {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Rect {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

export interface Params {
  /** Unrotated page sizes in points. */
  sizes: PageSize[];
  rotation: number;
  mode: PageMode;
  zoom: Zoom;
  viewWidth: number;
  viewHeight: number;
  /** The page shown in single mode. */
  current: number;
}

export interface Layout {
  /** CSS pixels per point. */
  scale: number;
  rotation: number;
  /** Placement per page; null for pages not shown (single mode). */
  frames: (Frame | null)[];
  /** Unrotated page sizes in points. */
  sizes: PageSize[];
  /** Page indices per row, top to bottom. */
  rows: number[][];
  width: number;
  height: number;
}

export function computeLayout(p: Params): Layout {
  const rotation = ((p.rotation % 360) + 360) % 360;
  const rotated = (i: number): [number, number] => {
    const { width, height } = p.sizes[i];
    return rotation % 180 === 0 ? [width, height] : [height, width];
  };
  const count = p.sizes.length;
  const pairs = (from: number) => {
    const rows: number[][] = [];
    for (let i = from; i < count; i += 2) rows.push(i + 1 < count ? [i, i + 1] : [i]);
    return rows;
  };
  let rows: number[][];
  if (count === 0) rows = [];
  else if (p.mode === "single") rows = [[Math.min(Math.max(p.current, 0), count - 1)]];
  else if (p.mode === "continuous") rows = p.sizes.map((_, i) => [i]);
  else if (p.mode === "two-up") rows = pairs(0);
  else rows = [[0], ...pairs(1)];

  // Row sizes in points; pages in a row sit side by side with GAP (pixels) between them.
  const rowPoints = (row: number[]) => {
    let w = 0;
    let h = 0;
    for (const i of row) {
      const [pw, ph] = rotated(i);
      w += pw;
      h = Math.max(h, ph);
    }
    return { w, h, gaps: (row.length - 1) * GAP };
  };
  let widest = { w: 1, h: 1, gaps: 0 };
  for (const row of rows) {
    const r = rowPoints(row);
    widest = {
      w: Math.max(widest.w, r.w),
      h: Math.max(widest.h, r.h),
      gaps: Math.max(widest.gaps, r.gaps),
    };
  }
  const availW = Math.max(p.viewWidth - 2 * MARGIN - widest.gaps, 1);
  const availH = Math.max(p.viewHeight - 2 * MARGIN, 1);
  let scale =
    p.zoom === "fit-width"
      ? availW / widest.w
      : p.zoom === "fit-page"
        ? Math.min(availW / widest.w, availH / widest.h)
        : p.zoom;
  // Fits round down a hair, so the column never overflows the view by a fraction of a pixel.
  if (typeof p.zoom !== "number") scale = Math.floor(scale * 10000) / 10000;
  scale = Math.min(Math.max(scale, MIN_SCALE), MAX_SCALE);

  let contentW = 0;
  for (const row of rows) {
    const r = rowPoints(row);
    contentW = Math.max(contentW, r.w * scale + r.gaps);
  }
  const width = Math.max(contentW + 2 * MARGIN, p.viewWidth);

  const frames: (Frame | null)[] = new Array(count).fill(null);
  let y = MARGIN;
  for (const row of rows) {
    const r = rowPoints(row);
    let x = (width - (r.w * scale + r.gaps)) / 2;
    for (const i of row) {
      const [pw, ph] = rotated(i);
      frames[i] = { x, y, width: pw * scale, height: ph * scale };
      x += pw * scale + GAP;
    }
    y += r.h * scale + GAP;
  }
  const height = rows.length ? y - GAP + MARGIN : 0;
  return { scale, rotation, frames, sizes: p.sizes, rows, width, height };
}

function rowTop(l: Layout, row: number[]): number {
  return Math.min(...row.map((i) => l.frames[i]?.y ?? Infinity));
}

function rowBottom(l: Layout, row: number[]): number {
  return Math.max(...row.map((i) => (l.frames[i] ? l.frames[i].y + l.frames[i].height : 0)));
}

/** Pages whose frames overlap the band [top, bottom], in order. */
export function visible(l: Layout, top: number, bottom: number): number[] {
  const out: number[] = [];
  for (const row of l.rows) {
    if (rowBottom(l, row) < top) continue;
    if (rowTop(l, row) > bottom) break;
    out.push(...row);
  }
  return out;
}

/** The page the reader is looking at: the one crossing a line a third of the way down the
 * view, else the nearest one. */
export function currentPage(l: Layout, top: number, viewHeight: number): number {
  const line = top + viewHeight / 3;
  let best = { distance: Infinity, page: 0 };
  l.frames.forEach((f, i) => {
    if (!f) return;
    const distance = line < f.y ? f.y - line : line > f.y + f.height ? line - (f.y + f.height) : 0;
    if (distance < best.distance) best = { distance, page: i };
  });
  return best.page;
}

/** Frame-relative CSS pixels to page space. */
export function frameToPage(l: Layout, page: number, fx: number, fy: number): [number, number] {
  const { width: w, height: h } = l.sizes[page];
  const [rx, ry] = [fx / l.scale, fy / l.scale];
  switch (l.rotation) {
    case 90:
      return [ry, h - rx];
    case 180:
      return [w - rx, h - ry];
    case 270:
      return [w - ry, rx];
    default:
      return [rx, ry];
  }
}

/** Page space to frame-relative CSS pixels; matches MuPDF's rotate-then-translate. */
export function pageToFrame(l: Layout, page: number, x: number, y: number): [number, number] {
  const { width: w, height: h } = l.sizes[page];
  let r: [number, number];
  switch (l.rotation) {
    case 90:
      r = [h - y, x];
      break;
    case 180:
      r = [w - x, h - y];
      break;
    case 270:
      r = [y, w - x];
      break;
    default:
      r = [x, y];
  }
  return [r[0] * l.scale, r[1] * l.scale];
}

/** A page-space rectangle in document space. */
export function toView(l: Layout, page: number, r: Rect): Frame | null {
  const f = l.frames[page];
  if (!f) return null;
  const [ax, ay] = pageToFrame(l, page, r.x0, r.y0);
  const [bx, by] = pageToFrame(l, page, r.x1, r.y1);
  return {
    x: f.x + Math.min(ax, bx),
    y: f.y + Math.min(ay, by),
    width: Math.abs(bx - ax),
    height: Math.abs(by - ay),
  };
}

/** The page under a document-space point, with the point in page space. */
export function hit(l: Layout, x: number, y: number): [number, number, number] | null {
  for (let i = 0; i < l.frames.length; i++) {
    const f = l.frames[i];
    if (f && x >= f.x && x <= f.x + f.width && y >= f.y && y <= f.y + f.height) {
      return [i, ...frameToPage(l, i, x - f.x, y - f.y)];
    }
  }
  return null;
}

/** The scroll top that shows `page`, at page-space height `top` when given. */
export function scrollTo(l: Layout, page: number, top?: number): number | null {
  const f = l.frames[page];
  if (!f) return null;
  const y = top === undefined ? f.y : (toView(l, page, { x0: 0, y0: top, x1: 0, y1: top })?.y ?? f.y);
  return Math.max(y - MARGIN / 2, 0);
}

/** The next zoom step from `scale` (CSS pixels per point) in direction `dir`. */
export function zoomStep(scale: number, dir: 1 | -1): number {
  const factor = scale / PX_PER_POINT;
  const next =
    dir > 0 ? ZOOMS.find((z) => z > factor * 1.01) : ZOOMS.findLast((z) => z < factor * 0.99);
  return (next ?? factor) * PX_PER_POINT;
}

/** Ctrl+wheel: about 12% per notch of 100 pixels, within the zoom steps' range. */
export function wheelZoom(scale: number, deltaY: number): number {
  const notches = Math.min(Math.max(-deltaY / 100, -3), 3);
  const factor = (scale * 1.12 ** notches) / PX_PER_POINT;
  return Math.min(Math.max(factor, ZOOMS[0]), ZOOMS[ZOOMS.length - 1]) * PX_PER_POINT;
}

/** A text line's box in CSS pixels on the unrotated page, for the selectable layer. */
export function lineBox(
  line: { x: number; y: number; w: number; h: number; size: number },
  scale: number,
): { left: number; top: number; width: number; height: number; fontSize: number } {
  return {
    left: line.x * scale,
    top: line.y * scale,
    width: line.w * scale,
    height: line.h * scale,
    fontSize: line.size * scale,
  };
}

/** The CSS transform that turns the unrotated page (`w` by `h` CSS pixels) into its frame. */
export function sheetTransform(rotation: number, w: number, h: number): string {
  switch (rotation) {
    case 90:
      return `translate(${h}px, 0) rotate(90deg)`;
    case 180:
      return `translate(${w}px, ${h}px) rotate(180deg)`;
    case 270:
      return `translate(0, ${w}px) rotate(270deg)`;
    default:
      return "none";
  }
}
