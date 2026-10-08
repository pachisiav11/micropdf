// Where pages sit in the scrolling column, and the zoom steps.

import type { PageSize } from "./pdf";

/** CSS pixels per PDF point at 100%: a point is 1/72 inch, a CSS pixel 1/96. */
export const PX_PER_POINT = 96 / 72;
/** Space between pages, and around the column, in CSS pixels. */
export const GAP = 16;
export const MARGIN = 24;

export const ZOOMS = [0.25, 0.33, 0.5, 0.67, 0.75, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4, 5];

/** Each page's top in CSS pixels at `scale` CSS pixels per point, and the column's size. */
export function pageTops(
  pages: PageSize[],
  scale: number,
): { tops: number[]; width: number; height: number } {
  if (!pages.length) return { tops: [], width: 0, height: 0 };
  const tops: number[] = [];
  let y = MARGIN;
  for (const p of pages) {
    tops.push(y);
    y += p.height * scale + GAP;
  }
  const widest = Math.max(...pages.map((p) => p.width));
  return { tops, width: widest * scale + 2 * MARGIN, height: y - GAP + MARGIN };
}

/** The index of the page that holds the line a third of the way down the view. */
export function pageAt(tops: number[], scrollTop: number, viewHeight: number): number {
  const line = scrollTop + viewHeight / 3;
  let lo = 0;
  let hi = tops.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (tops[mid] <= line) lo = mid;
    else hi = mid - 1;
  }
  return Math.max(lo, 0);
}

/** The zoom (1 is 100%) at which the widest page fills `width` CSS pixels, margins aside. */
export function fitWidth(pages: PageSize[], width: number): number {
  const widest = Math.max(1, ...pages.map((p) => p.width));
  // Rounded down, so the column never overflows the view by a fraction of a pixel.
  const zoom = Math.floor((1000 * (width - 2 * MARGIN)) / (widest * PX_PER_POINT)) / 1000;
  return Math.max(ZOOMS[0], zoom);
}

/** The next step from `zoom` in direction `dir` (+1 in, -1 out). */
export function zoomStep(zoom: number, dir: 1 | -1): number {
  const next = dir > 0 ? ZOOMS.find((z) => z > zoom + 1e-3) : ZOOMS.findLast((z) => z < zoom - 1e-3);
  return next ?? zoom;
}

/** A text line's box in CSS pixels at `scale`, for the selectable layer over a page. */
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
