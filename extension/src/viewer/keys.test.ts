import { describe, expect, it } from "vitest";
import { type KeyInput, mapKey, vimState } from "./keys";

const key = (k: string, mods: Partial<KeyInput> = {}): KeyInput => ({
  key: k,
  ctrl: false,
  shift: false,
  alt: false,
  ...mods,
});
const plain = { vim: false, presenting: false };
const vim = { vim: true, presenting: false };

describe("mapKey", () => {
  it("maps the desktop's Ctrl shortcuts and leaves the rest to the browser", () => {
    const s = vimState();
    expect(mapKey(key("f", { ctrl: true }), plain, s)).toEqual({ id: "find" });
    expect(mapKey(key("F", { ctrl: true, shift: true }), plain, s)).toEqual({ id: "find" });
    expect(mapKey(key("=", { ctrl: true }), plain, s)).toEqual({ id: "zoom-in" });
    expect(mapKey(key("+", { ctrl: true, shift: true }), plain, s)).toEqual({ id: "rotate-cw" });
    expect(mapKey(key("_", { ctrl: true, shift: true }), plain, s)).toEqual({ id: "rotate-ccw" });
    expect(mapKey(key("0", { ctrl: true }), plain, s)).toEqual({ id: "fit-page" });
    expect(mapKey(key("2", { ctrl: true }), plain, s)).toEqual({ id: "fit-width" });
    expect(mapKey(key("c", { ctrl: true }), plain, s)).toBeNull();
    expect(mapKey(key("t", { ctrl: true }), plain, s)).toBeNull();
  });

  it("scrolls and turns pages without modifiers", () => {
    const s = vimState();
    expect(mapKey(key("ArrowDown"), plain, s)).toEqual({ id: "scroll", x: 0, y: 1, unit: "line" });
    expect(mapKey(key(" "), plain, s)).toEqual({ id: "page-down" });
    expect(mapKey(key(" ", { shift: true }), plain, s)).toEqual({ id: "page-up" });
    expect(mapKey(key("End"), plain, s)).toEqual({ id: "last-page" });
    expect(mapKey(key("ArrowLeft", { alt: true }), plain, s)).toEqual({ id: "back" });
    expect(mapKey(key("F3", { shift: true }), plain, s)).toEqual({ id: "find-prev" });
  });

  it("turns pages with every forward key while presenting", () => {
    const s = vimState();
    const presenting = { vim: false, presenting: true };
    for (const k of ["ArrowRight", "ArrowDown", "PageDown", "Enter", " "]) {
      expect(mapKey(key(k), presenting, s)).toEqual({ id: "next-page", n: 1 });
    }
    expect(mapKey(key(" ", { shift: true }), presenting, s)).toEqual({ id: "prev-page", n: 1 });
  });

  it("ignores letters unless the Vim layer is on", () => {
    expect(mapKey(key("j"), plain, vimState())).toBeNull();
    expect(mapKey(key("j"), vim, vimState())).toEqual({ id: "scroll", x: 0, y: 1, unit: "line" });
  });

  it("takes Vim counts and the g prefix", () => {
    const s = vimState();
    expect(mapKey(key("1"), vim, s)).toEqual({ id: "none" });
    expect(mapKey(key("2"), vim, s)).toEqual({ id: "none" });
    expect(mapKey(key("G"), vim, s)).toEqual({ id: "go-page", page: 11 });
    expect(mapKey(key("G"), vim, s)).toEqual({ id: "last-page" });
    expect(mapKey(key("g"), vim, s)).toEqual({ id: "none" });
    expect(mapKey(key("g"), vim, s)).toEqual({ id: "go-page", page: 0 });
    expect(mapKey(key("3"), vim, s)).toEqual({ id: "none" });
    expect(mapKey(key("J"), vim, s)).toEqual({ id: "next-page", n: 3 });
    expect(mapKey(key("0"), vim, s)).toBeNull();
  });

  it("forgets a count and a prefix on Escape", () => {
    const s = vimState();
    mapKey(key("5"), vim, s);
    mapKey(key("g"), vim, s);
    expect(mapKey(key("Escape"), vim, s)).toEqual({ id: "escape" });
    expect(s).toEqual({ count: "", pending: null });
  });
});
