import { describe, expect, it } from "vitest";
import { DEFAULT_COLORS, INK_WIDTH, LINE_WIDTH, draft, spansOf } from "./annotate";
import type { Rgb } from "./pdf";

const red: Rgb = [1, 0, 0];

describe("draft", () => {
  it("puts a note where the press was", () => {
    expect(draft("note", [[10, 20]], red, "hi")).toEqual({ kind: "note", at: [10, 20], text: "hi", color: red });
  });

  it("makes shapes from either drag direction", () => {
    expect(
      draft(
        "square",
        [
          [50, 60],
          [10, 20],
        ],
        red,
        "",
      ),
    ).toEqual({ kind: "square", rect: { x0: 10, y0: 20, x1: 50, y1: 60 }, color: red, width: LINE_WIDTH });
    expect(
      draft(
        "arrow",
        [
          [0, 0],
          [30, 0],
        ],
        red,
        "",
      ),
    ).toMatchObject({ kind: "line", from: [0, 0], to: [30, 0], arrow: true });
  });

  it("ignores clicks for tools that need a drag", () => {
    const click: [number, number][] = [
      [10, 10],
      [11, 12],
    ];
    expect(draft("square", click, red, "")).toBeNull();
    expect(draft("text", click, red, "x")).toBeNull();
    expect(draft("ink", [[10, 10]], red, "")).toBeNull();
  });

  it("keeps every point of a pen stroke", () => {
    const stroke: [number, number][] = [
      [0, 0],
      [1, 1],
      [2, 0],
    ];
    expect(draft("ink", stroke, DEFAULT_COLORS.ink, "")).toEqual({
      kind: "ink",
      strokes: [stroke],
      color: DEFAULT_COLORS.ink,
      width: INK_WIDTH,
    });
  });
});

it("marks each selected line across its middle", () => {
  expect(spansOf([{ x0: 10, y0: 100, x1: 90, y1: 112 }])).toEqual([
    [
      [10, 106],
      [90, 106],
    ],
  ]);
});
