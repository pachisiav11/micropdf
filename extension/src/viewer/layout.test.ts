import { describe, expect, it } from "vitest";
import { GAP, MARGIN, PX_PER_POINT, fitWidth, lineBox, pageAt, pageTops, zoomStep } from "./layout";

const pages = [
  { width: 612, height: 792 },
  { width: 612, height: 792 },
  { width: 792, height: 612 },
];

describe("pageTops", () => {
  it("stacks pages with gaps inside margins", () => {
    const { tops, width, height } = pageTops(pages, 1);
    expect(tops).toEqual([MARGIN, MARGIN + 792 + GAP, MARGIN + 2 * (792 + GAP)]);
    expect(width).toBe(MARGIN + 792 + MARGIN);
    expect(height).toBe(MARGIN + 792 * 2 + 612 + 2 * GAP + MARGIN);
    expect(pageTops([], 1)).toEqual({ tops: [], width: 0, height: 0 });
  });
});

describe("pageAt", () => {
  it("finds the page a third of the way down", () => {
    const { tops } = pageTops(pages, 1);
    expect(pageAt(tops, 0, 900)).toBe(0);
    expect(pageAt(tops, tops[1] - 300 + 1, 900)).toBe(1);
    expect(pageAt(tops, 1e6, 900)).toBe(2);
  });
});

describe("zoom", () => {
  it("fits the widest page", () => {
    expect(fitWidth(pages, 2 * MARGIN + 792 * PX_PER_POINT)).toBeCloseTo(1);
  });

  it("steps through the list and stops at the ends", () => {
    expect(zoomStep(1, 1)).toBe(1.1);
    expect(zoomStep(1, -1)).toBe(0.9);
    expect(zoomStep(1.05, 1)).toBe(1.1);
    expect(zoomStep(5, 1)).toBe(5);
    expect(zoomStep(0.25, -1)).toBe(0.25);
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
