// The page's side of the mupdf.js worker. Requests wait in a queue and go to the worker one at a
// time, most urgent first, so the pages in view draw before thumbnails and search; a request
// whose page has scrolled away is dropped before it costs anything.

import type { Rect } from "./layout";
import type { ReadingMode } from "./recolor";

export interface PageSize {
  width: number;
  height: number;
}

export interface Opened {
  needsPassword: boolean;
  pages: PageSize[];
  title: string;
}

/** A page, or part of one, drawn as RGBA pixels; `x` and `y` place it on the whole page. */
export interface Drawn {
  x: number;
  y: number;
  width: number;
  height: number;
  pixels: Uint8ClampedArray<ArrayBuffer>;
}

/** One line of a page's text, in PDF points from the page's top left. */
export interface TextLine {
  x: number;
  y: number;
  w: number;
  h: number;
  size: number;
  text: string;
}

/** Where a link or bookmark leads: a page (and height on it) in this document, or a URI. */
export interface Target {
  page?: number;
  top?: number;
  uri?: string;
}

export interface OutlineNode extends Target {
  title: string;
  open: boolean;
  children: OutlineNode[];
}

export interface PageLink extends Target {
  rect: Rect;
}

/** One search hit: a rectangle per line it covers, in page space. */
export type Hit = Rect[];

export type Point = [number, number];
export type Rgb = [number, number, number];

/** A comment the reader adds; points and rectangles are in page space. */
export type NewAnnot =
  | { kind: "note"; at: Point; text: string; color: Rgb }
  | { kind: "text"; rect: Rect; text: string; color: Rgb; size: number }
  | { kind: "ink"; strokes: Point[][]; color: Rgb; width: number }
  | { kind: "square" | "circle"; rect: Rect; color: Rgb; width: number }
  | { kind: "line"; from: Point; to: Point; arrow: boolean; color: Rgb; width: number }
  /** Text from `from` to `to` in reading order, as a selection would take it. */
  | { kind: "highlight" | "underline" | "strikeout"; spans: [Point, Point][]; color: Rgb };

export interface AnnotInfo {
  /** The annotation's object number. */
  id: number;
  type: string;
  rect: Rect;
  contents: string;
  author: string;
  locked: boolean;
}

export interface FieldInfo {
  /** The widget's object number. */
  id: number;
  type: "text" | "checkbox" | "radiobutton" | "combobox" | "listbox" | "button" | "signature";
  name: string;
  rect: Rect;
  value: string;
  /** Checkboxes and radio buttons: whether this widget is on. */
  checked: boolean;
  options: string[];
  readOnly: boolean;
  multiline: boolean;
  maxLen: number;
}

/** What an edit changed: the pages to draw again (all of them when null), and the history. */
export interface Edited {
  pages: number[] | null;
  canUndo: boolean;
  canRedo: boolean;
}

export type Request =
  | { method: "open"; data: ArrayBuffer }
  | { method: "unlock"; password: string }
  | { method: "outline" }
  | { method: "render"; page: number; scale: number; mode: ReadingMode; clip?: Rect }
  | { method: "text"; page: number }
  | { method: "links"; page: number }
  | { method: "search"; page: number; needle: string }
  | { method: "annotAt"; page: number; at: Point }
  | { method: "addAnnot"; page: number; annot: NewAnnot; author: string }
  | { method: "editAnnot"; page: number; id: number; contents?: string; color?: Rgb }
  | { method: "deleteAnnot"; page: number; id: number }
  | { method: "fields"; page: number }
  | { method: "setField"; page: number; id: number; value: string }
  | { method: "toggleField"; page: number; id: number }
  | { method: "undo" }
  | { method: "redo" }
  | { method: "save" }
  | { method: "copy" };

export interface Saved {
  bytes: Uint8Array<ArrayBuffer>;
  canUndo: boolean;
  canRedo: boolean;
}

export interface Call {
  id: number;
  request: Request;
}

export type Reply = { id: number; ok: true; result: unknown } | { id: number; ok: false; error: string };

/** How soon a request goes: higher first. */
export const Priority = { Background: 0, Page: 1, Visible: 2, Document: 3 } as const;
export type Priority = (typeof Priority)[keyof typeof Priority];

/** The rejection for a request that was no longer wanted when its turn came. */
export class Dropped extends Error {
  constructor() {
    super("dropped");
  }
}

interface Waiting {
  id: number;
  request: Request;
  transfer: Transferable[];
  priority: Priority;
  wanted?: () => boolean;
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
}

export class Pdf {
  private worker = new Worker(new URL("./pdf.worker.ts", import.meta.url), { type: "module" });
  private next = 1;
  private queue: Waiting[] = [];
  private running: Waiting | null = null;

  constructor() {
    this.worker.onmessage = (e: MessageEvent<Reply>) => {
      const reply = e.data;
      const done = this.running;
      this.running = null;
      if (done && done.id === reply.id) {
        if (reply.ok) done.resolve(reply.result);
        else done.reject(new Error(reply.error));
      }
      this.pump();
    };
  }

  private call<T>(
    request: Request,
    priority: Priority,
    wanted?: () => boolean,
    transfer: Transferable[] = [],
  ): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      this.queue.push({
        id: this.next++,
        request,
        transfer,
        priority,
        wanted,
        resolve: resolve as (v: unknown) => void,
        reject,
      });
      this.pump();
    });
  }

  private pump(): void {
    while (!this.running && this.queue.length) {
      let best = 0;
      for (let i = 1; i < this.queue.length; i++) {
        if (this.queue[i].priority > this.queue[best].priority) best = i;
      }
      const [job] = this.queue.splice(best, 1);
      if (job.wanted && !job.wanted()) {
        job.reject(new Dropped());
        continue;
      }
      this.running = job;
      // The request stays apart from the call's id: requests carry ids of their own.
      this.worker.postMessage({ id: job.id, request: job.request } satisfies Call, job.transfer);
    }
  }

  /** Opens the PDF in `data`, which moves to the worker. */
  open(data: ArrayBuffer): Promise<Opened> {
    return this.call({ method: "open", data }, Priority.Document, undefined, [data]);
  }

  /** Tries `password`; the pages once it is right, else null. */
  unlock(password: string): Promise<Opened | null> {
    return this.call({ method: "unlock", password }, Priority.Document);
  }

  outline(): Promise<OutlineNode[]> {
    return this.call({ method: "outline" }, Priority.Document);
  }

  /** Draws page `page`, or only `clip` (page space) of it, at `scale` device pixels per point. */
  render(
    page: number,
    scale: number,
    mode: ReadingMode,
    clip: Rect | undefined,
    priority: Priority,
    wanted?: () => boolean,
  ): Promise<Drawn> {
    return this.call({ method: "render", page, scale, mode, clip }, priority, wanted);
  }

  text(page: number, wanted?: () => boolean): Promise<TextLine[]> {
    return this.call({ method: "text", page }, Priority.Page, wanted);
  }

  links(page: number, wanted?: () => boolean): Promise<PageLink[]> {
    return this.call({ method: "links", page }, Priority.Page, wanted);
  }

  search(page: number, needle: string, wanted?: () => boolean): Promise<Hit[]> {
    return this.call({ method: "search", page, needle }, Priority.Background, wanted);
  }

  /** The topmost comment at `at`, or null. */
  annotAt(page: number, at: Point): Promise<AnnotInfo | null> {
    return this.call({ method: "annotAt", page, at }, Priority.Document);
  }

  addAnnot(page: number, annot: NewAnnot, author: string): Promise<Edited & { id: number }> {
    return this.call({ method: "addAnnot", page, annot, author }, Priority.Document);
  }

  editAnnot(page: number, id: number, change: { contents?: string; color?: Rgb }): Promise<Edited> {
    return this.call({ method: "editAnnot", page, id, ...change }, Priority.Document);
  }

  deleteAnnot(page: number, id: number): Promise<Edited> {
    return this.call({ method: "deleteAnnot", page, id }, Priority.Document);
  }

  fields(page: number, wanted?: () => boolean): Promise<FieldInfo[]> {
    return this.call({ method: "fields", page }, Priority.Page, wanted);
  }

  setField(page: number, id: number, value: string): Promise<Edited> {
    return this.call({ method: "setField", page, id, value }, Priority.Document);
  }

  toggleField(page: number, id: number): Promise<Edited> {
    return this.call({ method: "toggleField", page, id }, Priority.Document);
  }

  undo(): Promise<Edited> {
    return this.call({ method: "undo" }, Priority.Document);
  }

  redo(): Promise<Edited> {
    return this.call({ method: "redo" }, Priority.Document);
  }

  /** The document's bytes with every change, appended to the original when possible; the undo
   * history starts again after it. */
  save(): Promise<Saved> {
    return this.call({ method: "save" }, Priority.Document);
  }

  /** The document's bytes with every change, for printing; the document stays as it is. */
  copy(): Promise<Uint8Array<ArrayBuffer>> {
    return this.call({ method: "copy" }, Priority.Document);
  }
}
