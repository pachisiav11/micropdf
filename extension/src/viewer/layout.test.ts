import { describe, expect, it } from "vitest";
import {
  GAP,
  MARGIN,
  PX_PER_POINT,
  type PageMode,
  type Params,
  type Zoom,
  computeLayout,
  currentPage,
  hit,
  lineBox,
  scrollTo,
  sheetTransform,
  toView,
  visible,
  wheelZoom,
  zoomStep,
} from "./layout";

const LETTER = { width: 612, height: 792 };

function params(n: number, mode: PageMode, zoom: Zoom, rotation = 0): Params {
  return {
    sizes: Array.from({ length: n }, () => LETTER),
    rotation,
    mode,
    zoom,
    viewWidth: 848,
    viewHeight: 600,
    current: 0,
  };
}

describe("computeLayout", () => {
  it("fits the width minus the margins", () => {
    const l = computeLayout(params(3, "continuous", "fit-width"));
    expect(l.scale).toBeCloseTo(800 / 612, 3);
    expect(l.scale * 612 + 2 * MARGIN).toBeLessThanOrEqual(848);
    const [f0, f1, f2] = l.frames;
    expect(f0!.y).toBe(MARGIN);
    expect(f1!.y).toBeCloseTo(f0!.y + f0!.height + GAP);
    expect(l.height).toBeCloseTo(f2!.y + f2!.height + MARGIN);
    expect(l.width).toBe(848);
  });

  it("fits the tallest page", () => {
    const l = computeLayout(params(1, "continuous", "fit-page"));
    expect(l.frames[0]!.height).toBeCloseTo(552, 0);
    expect(l.frames[0]!.height).toBeLessThanOrEqual(552);
  });

  it("pairs pages in two-up and book modes", () => {
    const two = computeLayout(params(5, "two-up", 0.5));
    expect(two.frames[0]!.y).toBe(two.frames[1]!.y);
    expect(two.frames[2]!.y).toBeGreaterThan(two.frames[1]!.y);
    expect(two.rows).toEqual([[0, 1], [2, 3], [4]]);

    const book = computeLayout(params(5, "book", 0.5));
    expect(book.rows).toEqual([[0], [1, 2], [3, 4]]);
    expect(book.frames[1]!.y).toBe(book.frames[2]!.y);
  });

  it("places only the current page in single mode", () => {
    const l = computeLayout({ ...params(4, "single", "fit-page"), current: 2 });
    expect(l.frames.map((f) => f !== null)).toEqual([false, false, true, false]);
    expect(visible(l, 0, 10_000)).toEqual([2]);
  });

  it("swaps the frame size when rotated", () => {
    const l = computeLayout({ ...params(1, "continuous", 1, 90), sizes: [{ width: 300, height: 200 }] });
    expect([l.frames[0]!.width, l.frames[0]!.height]).toEqual([200, 300]);
  });

  it("grows wider than the view when zoomed in, with pages centred", () => {
    const l = computeLayout(params(1, "continuous", 2));
    expect(l.width).toBe(612 * 2 + 2 * MARGIN);
    expect(l.frames[0]!.x).toBe(MARGIN);
  });

  it("is empty without pages", () => {
    const l = computeLayout(params(0, "continuous", "fit-width"));
    expect(l.height).toBe(0);
    expect(visible(l, 0, 1000)).toEqual([]);
  });
});

describe("coordinates", () => {
  it("round-trip between page and view for every rotation", () => {
    for (const rotation of [0, 90, 180, 270]) {
      const l = computeLayout({
        ...params(1, "continuous", 1.5, rotation),
        sizes: [{ width: 300, height: 200 }],
      });
      const v = toView(l, 0, { x0: 40, y0: 30, x1: 41, y1: 31 })!;
      const [page, x, y] = hit(l, v.x + v.width / 2, v.y + v.height / 2)!;
      expect(page).toBe(0);
      expect(x).toBeCloseTo(40.5, 2);
      expect(y).toBeCloseTo(30.5, 2);
    }
  });

  it("rotates the sheet onto its frame", () => {
    expect(sheetTransform(0, 300, 200)).toBe("none");
    expect(sheetTransform(90, 300, 200)).toBe("translate(200px, 0) rotate(90deg)");
    expect(sheetTransform(270, 300, 200)).toBe("translate(0, 300px) rotate(270deg)");
  });
});

describe("scrolling", () => {
  it("follows the scroll position", () => {
    const l = computeLayout(params(10, "continuous", 1));
    const f3 = l.frames[3]!;
    expect(visible(l, f3.y + 10, f3.y + 20)).toEqual([3]);
    expect(visible(l, f3.y + f3.height - 1, f3.y + f3.height + GAP + 1)).toEqual([3, 4]);
    expect(currentPage(l, f3.y, 600)).toBe(3);
    expect(scrollTo(l, 3)).toBe(f3.y - MARGIN / 2);
    expect(scrollTo(l, 3, 100)).toBe(f3.y + 100 - MARGIN / 2);
  });
});

describe("zoom", () => {
  it("steps through the list and stops at the ends", () => {
    expect(zoomStep(PX_PER_POINT, 1) / PX_PER_POINT).toBeCloseTo(1.1);
    expect(zoomStep(PX_PER_POINT, -1) / PX_PER_POINT).toBeCloseTo(0.9);
    expect(zoomStep(1.05 * PX_PER_POINT, 1) / PX_PER_POINT).toBeCloseTo(1.1);
    expect(zoomStep(12 * PX_PER_POINT, 1) / PX_PER_POINT).toBeCloseTo(12);
    expect(zoomStep(0.1 * PX_PER_POINT, -1) / PX_PER_POINT).toBeCloseTo(0.1);
  });

  it("zooms about 12% per wheel notch, within the steps' range", () => {
    expect(wheelZoom(PX_PER_POINT, -100) / PX_PER_POINT).toBeCloseTo(1.12);
    expect(wheelZoom(PX_PER_POINT, 100) / PX_PER_POINT).toBeCloseTo(1 / 1.12);
    expect(wheelZoom(12 * PX_PER_POINT, -300) / PX_PER_POINT).toBe(12);
  });
});

describe("lineBox", () => {
  it("scales a line to CSS pixels", () => {
    expect(lineBox({ x: 72, y: 76, w: 41, h: 19, size: 14 }, 2)).toEqual({
      left: 144,
      top: 152,
      width: 82,
      height: 38,
      fontSize: 28,
    });
  });
});
