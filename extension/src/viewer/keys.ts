// Keyboard commands, as on the desktop (app/src/commands.rs): the same shortcuts and the same
// optional Vim layer. Pure, so the mapping can be tested without a page.

export type Simple =
  | "none"
  | "find"
  | "goto"
  | "copy"
  | "zoom-in"
  | "zoom-out"
  | "fit-page"
  | "fit-width"
  | "zoom-100"
  | "rotate-cw"
  | "rotate-ccw"
  | "first-page"
  | "last-page"
  | "page-down"
  | "page-up"
  | "left"
  | "right"
  | "back"
  | "forward"
  | "find-next"
  | "find-prev"
  | "sidebar"
  | "present"
  | "undo"
  | "redo"
  | "save"
  | "save-as"
  | "print"
  | "delete"
  | "assistant"
  | "escape";

export type Command =
  | { id: Simple }
  | { id: "scroll"; x: number; y: number; unit: "line" | "half" }
  | { id: "next-page" | "prev-page"; n: number }
  /** A page index, from 0. */
  | { id: "go-page"; page: number };

export interface KeyInput {
  /** KeyboardEvent.key. */
  key: string;
  /** Ctrl, or Cmd on a Mac. */
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
}

/** Vim's count and pending prefix between keys. */
export interface VimState {
  count: string;
  pending: string | null;
}

export function vimState(): VimState {
  return { count: "", pending: null };
}

const CTRL: Record<string, Simple> = {
  f: "find",
  g: "goto",
  l: "present",
  "=": "zoom-in",
  "-": "zoom-out",
  _: "rotate-ccw",
  "0": "fit-page",
  "1": "zoom-100",
  "2": "fit-width",
  z: "undo",
  y: "redo",
  s: "save",
  p: "print",
  Home: "first-page",
  End: "last-page",
};

/** The command for a key press, or null to leave it to the browser. */
export function mapKey(
  k: KeyInput,
  context: { vim: boolean; presenting: boolean },
  vim: VimState,
): Command | null {
  const c = (id: Simple): Command => ({ id });
  if (k.ctrl) {
    if (k.alt) return null;
    if (k.key === "+") return c(k.shift ? "rotate-cw" : "zoom-in");
    const id = CTRL[k.key.length === 1 ? k.key.toLowerCase() : k.key];
    if (id === "undo" && k.shift) return c("redo");
    if (id === "save" && k.shift) return c("save-as");
    if (k.shift && k.key.toLowerCase() === "a") return c("assistant");
    return id ? c(id) : null;
  }
  if (k.alt) {
    if (k.key === "ArrowLeft") return c("back");
    if (k.key === "ArrowRight") return c("forward");
    return null;
  }
  if (k.key === "Escape") {
    vim.count = "";
    vim.pending = null;
    return c("escape");
  }
  if (k.key === "F3") return c(k.shift ? "find-prev" : "find-next");
  if (k.key === "F4") return c("sidebar");
  if (k.key === "Delete") return c("delete");

  if (context.presenting) {
    const next = ["ArrowRight", "ArrowDown", "PageDown", "Enter"].includes(k.key);
    if (next || (k.key === " " && !k.shift)) return { id: "next-page", n: 1 };
    if (["ArrowLeft", "ArrowUp", "PageUp", " "].includes(k.key)) return { id: "prev-page", n: 1 };
  }

  switch (k.key) {
    case "ArrowDown":
      return { id: "scroll", x: 0, y: 1, unit: "line" };
    case "ArrowUp":
      return { id: "scroll", x: 0, y: -1, unit: "line" };
    case "PageDown":
      return c("page-down");
    case "PageUp":
      return c("page-up");
    case " ":
      return c(k.shift ? "page-up" : "page-down");
    case "Home":
      return c("first-page");
    case "End":
      return c("last-page");
    case "ArrowLeft":
      return c("left");
    case "ArrowRight":
      return c("right");
  }
  return context.vim && [...k.key].length === 1 ? vimKey(k.key, vim) : null;
}

function vimKey(key: string, vim: VimState): Command | null {
  if (/^[0-9]$/.test(key) && (key !== "0" || vim.count)) {
    if (vim.count.length < 6) vim.count += key;
    return { id: "none" };
  }
  const count = vim.count ? Number(vim.count) : null;
  vim.count = "";
  const n = Math.max(count ?? 1, 1);
  const prefix = vim.pending;
  vim.pending = null;
  if (prefix === "g") {
    return key === "g" ? { id: "go-page", page: count ? count - 1 : 0 } : { id: "none" };
  }
  switch (key) {
    case "j":
      return { id: "scroll", x: 0, y: n, unit: "line" };
    case "k":
      return { id: "scroll", x: 0, y: -n, unit: "line" };
    case "h":
      return { id: "scroll", x: -n, y: 0, unit: "line" };
    case "l":
      return { id: "scroll", x: n, y: 0, unit: "line" };
    case "d":
      return { id: "scroll", x: 0, y: n, unit: "half" };
    case "u":
      return { id: "scroll", x: 0, y: -n, unit: "half" };
    case "J":
      return { id: "next-page", n };
    case "K":
      return { id: "prev-page", n };
    case "G":
      return count ? { id: "go-page", page: count - 1 } : { id: "last-page" };
    case "g":
      vim.pending = "g";
      return { id: "none" };
  }
  const simple: Record<string, Simple> = {
    H: "back",
    L: "forward",
    n: "find-next",
    N: "find-prev",
    "/": "find",
    ":": "goto",
    "+": "zoom-in",
    "=": "zoom-in",
    "-": "zoom-out",
    s: "fit-width",
    a: "fit-page",
    r: "rotate-cw",
    R: "rotate-ccw",
    y: "copy",
  };
  return simple[key] ? { id: simple[key] } : null;
}
