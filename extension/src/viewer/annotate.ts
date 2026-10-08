// Comment tools, with the desktop's defaults (app/src/viewer.rs): the same swatches, colours
// and widths. Pure, so the shapes a drag makes can be tested without a page.

import type { Rect } from "./layout";
import type { NewAnnot, Point, Rgb } from "./pdf";

export type Markup = "highlight" | "underline" | "strikeout";
export type DrawTool = "note" | "text" | "square" | "circle" | "line" | "arrow" | "ink";
export type Tool = "select" | Markup | DrawTool;

export const TOOLS: [Tool, string][] = [
  ["select", "Select"],
  ["highlight", "Highlight"],
  ["underline", "Underline"],
  ["strikeout", "Strike out"],
  ["note", "Note"],
  ["text", "Text box"],
  ["square", "Rectangle"],
  ["circle", "Ellipse"],
  ["line", "Line"],
  ["arrow", "Arrow"],
  ["ink", "Pen"],
];

const YELLOW: Rgb = [1, 0.85, 0];
const RED: Rgb = [0.85, 0.15, 0.15];
const BLUE: Rgb = [0.1, 0.45, 0.9];

export const SWATCHES: [string, Rgb][] = [
  ["Yellow", YELLOW],
  ["Orange", [1, 0.55, 0.1]],
  ["Red", RED],
  ["Pink", [0.95, 0.4, 0.7]],
  ["Purple", [0.55, 0.3, 0.85]],
  ["Blue", BLUE],
  ["Green", [0.2, 0.7, 0.3]],
  ["Black", [0, 0, 0]],
];

/** What each tool draws with until the reader picks another colour. */
export const DEFAULT_COLORS: Record<Exclude<Tool, "select">, Rgb> = {
  highlight: YELLOW,
  underline: BLUE,
  strikeout: RED,
  note: YELLOW,
  text: RED,
  square: RED,
  circle: RED,
  line: RED,
  arrow: RED,
  ink: [0.1, 0.35, 0.9],
};

export const LINE_WIDTH = 1.5;
export const INK_WIDTH = 2;
export const TEXT_SIZE = 12;

export const isMarkup = (t: Tool): t is Markup => t === "highlight" || t === "underline" || t === "strikeout";
export const isDraw = (t: Tool): t is DrawTool => t !== "select" && !isMarkup(t);
export const needsText = (t: Tool) => t === "note" || t === "text";

export const css = ([r, g, b]: Rgb) => `rgb(${r * 255} ${g * 255} ${b * 255})`;

export function rectOf(a: Point, b: Point): Rect {
  return { x0: Math.min(a[0], b[0]), y0: Math.min(a[1], b[1]), x1: Math.max(a[0], b[0]), y1: Math.max(a[1], b[1]) };
}

/** The comment a drag over `points` (page space) makes; null when it is too small to be one. A
 * note goes where the press was; the rest need a drag. */
export function draft(tool: DrawTool, points: Point[], color: Rgb, text: string): NewAnnot | null {
  const [a, b] = [points[0], points[points.length - 1]];
  if (!a) return null;
  if (tool === "note") return { kind: "note", at: a, text, color };
  if (tool === "ink") return points.length > 1 ? { kind: "ink", strokes: [points], color, width: INK_WIDTH } : null;
  const rect = rectOf(a, b);
  if (rect.x1 - rect.x0 < 4 && rect.y1 - rect.y0 < 4) return null;
  switch (tool) {
    case "text":
      return { kind: "text", rect, text, color, size: TEXT_SIZE };
    case "square":
    case "circle":
      return { kind: tool, rect, color, width: LINE_WIDTH };
    default:
      return { kind: "line", from: a, to: b, arrow: tool === "arrow", color, width: LINE_WIDTH };
  }
}

/** The text to mark on each selected line: from the middle of its left edge to its right. */
export function spansOf(lines: Rect[]): [Point, Point][] {
  return lines.map((r) => {
    const y = (r.y0 + r.y1) / 2;
    return [
      [r.x0, y],
      [r.x1, y],
    ];
  });
}
