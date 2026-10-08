/** FNV-1a (64-bit) over the bytes as 16 hex digits: mp-ai's content_hash, so a PDF keeps one
 * assistant chat in the viewer and the desktop app. Two 32-bit halves, since a 64-bit multiply
 * in BigInt would be far slower over a whole file. */
export function contentHash(bytes: Uint8Array): string {
  let hi = 0xcbf29ce4;
  let lo = 0x84222325;
  for (let i = 0; i < bytes.length; i++) {
    lo = (lo ^ bytes[i]) >>> 0;
    // The prime is 2^40 + 0x1b3: lo * 0x1b3 stays below 2^41, so it is exact as a double.
    const low = lo * 0x1b3;
    hi = (Math.imul(hi, 0x1b3) + Math.floor(low / 0x100000000) + (lo << 8)) >>> 0;
    lo = low >>> 0;
  }
  return hi.toString(16).padStart(8, "0") + lo.toString(16).padStart(8, "0");
}
