// Reading modes, as on the desktop (app/src/recolor.rs): white paper becomes the paper colour and
// black ink the ink colour, linearly in between, per channel.

export type ReadingMode = "normal" | "dark" | "sepia" | "invert";

type Rgb = [number, number, number];

export const PAPER: Record<ReadingMode, Rgb> = {
  normal: [255, 255, 255],
  // Recto's dark sheet.
  dark: [0x1d, 0x20, 0x27],
  sepia: [0xf4, 0xec, 0xd8],
  invert: [0, 0, 0],
};

const INK: Record<ReadingMode, Rgb> = {
  normal: [0, 0, 0],
  dark: [0xe3, 0xe7, 0xef],
  sepia: [0x5b, 0x46, 0x36],
  invert: [255, 255, 255],
};

/** One lookup table per channel, or null when the mode leaves colours alone. */
export function tables(mode: ReadingMode): [Uint8Array, Uint8Array, Uint8Array] | null {
  if (mode === "normal") return null;
  const [paper, ink] = [PAPER[mode], INK[mode]];
  const table = (c: number) =>
    Uint8Array.from({ length: 256 }, (_, v) => Math.round(ink[c] + ((paper[c] - ink[c]) * v) / 255));
  return [table(0), table(1), table(2)];
}

export function paperCss(mode: ReadingMode): string {
  return `rgb(${PAPER[mode].join(" ")})`;
}

/** Packs MuPDF's RGB rows (`stride` bytes apart) into opaque RGBA, recoloured for `mode`. */
export function toRgba(
  rgb: ArrayLike<number>,
  width: number,
  height: number,
  stride: number,
  mode: ReadingMode,
): Uint8ClampedArray<ArrayBuffer> {
  const t = tables(mode);
  const out = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y++) {
    let s = y * stride;
    let d = y * width * 4;
    for (let x = 0; x < width; x++, s += 3, d += 4) {
      out[d] = t ? t[0][rgb[s]] : rgb[s];
      out[d + 1] = t ? t[1][rgb[s + 1]] : rgb[s + 1];
      out[d + 2] = t ? t[2][rgb[s + 2]] : rgb[s + 2];
      out[d + 3] = 255;
    }
  }
  return out;
}
