import { describe, expect, it } from "vitest";
import { paperCss, toRgba } from "./recolor";

describe("toRgba", () => {
  const paperAndInk = Uint8Array.from([255, 255, 255, 0, 0, 0]);

  it("maps paper and ink to the mode's colours", () => {
    expect([...toRgba(paperAndInk, 2, 1, 6, "dark")]).toEqual([
      0x1d, 0x20, 0x27, 255, 0xe3, 0xe7, 0xef, 255,
    ]);
    expect([...toRgba(paperAndInk, 2, 1, 6, "invert")]).toEqual([0, 0, 0, 255, 255, 255, 255, 255]);
  });

  it("leaves colours alone in normal mode and skips row padding", () => {
    const rgb = Uint8Array.from([10, 20, 30, 99, 40, 50, 60, 99]);
    expect([...toRgba(rgb, 1, 2, 4, "normal")]).toEqual([10, 20, 30, 255, 40, 50, 60, 255]);
  });

  it("gives the paper colour for CSS", () => {
    expect(paperCss("sepia")).toBe("rgb(244 236 216)");
  });
});
