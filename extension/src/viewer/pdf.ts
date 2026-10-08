// The page's side of the mupdf.js worker: one promise per request.

export interface PageSize {
  width: number;
  height: number;
}

export interface Opened {
  needsPassword: boolean;
  pages: PageSize[];
  title: string;
}

/** A page drawn as RGBA pixels. */
export interface Drawn {
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

export type Request =
  | { method: "open"; data: ArrayBuffer }
  | { method: "unlock"; password: string }
  | { method: "render"; page: number; scale: number }
  | { method: "text"; page: number };

export type Reply = { id: number } & ({ ok: true; result: unknown } | { ok: false; error: string });

export class Pdf {
  private worker = new Worker(new URL("./pdf.worker.ts", import.meta.url), { type: "module" });
  private next = 1;
  private pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();

  constructor() {
    this.worker.onmessage = (e: MessageEvent<Reply>) => {
      const reply = e.data;
      const waiting = this.pending.get(reply.id);
      this.pending.delete(reply.id);
      if (!waiting) return;
      if (reply.ok) waiting.resolve(reply.result);
      else waiting.reject(new Error(reply.error));
    };
  }

  private call<T>(request: Request, transfer: Transferable[] = []): Promise<T> {
    const id = this.next++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      this.worker.postMessage({ id, ...request }, transfer);
    });
  }

  /** Opens the PDF in `data`, which moves to the worker. */
  open(data: ArrayBuffer): Promise<Opened> {
    return this.call({ method: "open", data }, [data]);
  }

  /** Tries `password`; the pages once it is right, else null. */
  unlock(password: string): Promise<Opened | null> {
    return this.call({ method: "unlock", password });
  }

  /** Draws page `page` at `scale` device pixels per point. */
  render(page: number, scale: number): Promise<Drawn> {
    return this.call({ method: "render", page, scale });
  }

  text(page: number): Promise<TextLine[]> {
    return this.call({ method: "text", page });
  }
}
