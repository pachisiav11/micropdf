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

export type Request =
  | { method: "open"; data: ArrayBuffer }
  | { method: "unlock"; password: string }
  | { method: "outline" }
  | { method: "render"; page: number; scale: number; mode: ReadingMode; clip?: Rect }
  | { method: "text"; page: number }
  | { method: "links"; page: number }
  | { method: "search"; page: number; needle: string };

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
      this.worker.postMessage({ id: job.id, ...job.request }, job.transfer);
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
}
