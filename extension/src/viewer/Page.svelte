<script module lang="ts">
  // One canvas measures every text line, to stretch it over the line it covers on the page.
  let measure: CanvasRenderingContext2D | null = null;

  function stretch(text: string, width: number, fontSize: number): number {
    measure ??= document.createElement("canvas").getContext("2d");
    if (!measure) return 1;
    measure.font = `${fontSize}px sans-serif`;
    const natural = measure.measureText(text).width;
    return natural > 0 ? width / natural : 1;
  }

  /** The most device pixels a whole-page canvas holds; past it, a sharp tile covers the view. */
  const MAX_PIXELS = 16_000_000;

  function paint(canvas: HTMLCanvasElement, d: Drawn): void {
    canvas.width = d.width;
    canvas.height = d.height;
    canvas.getContext("2d")?.putImageData(new ImageData(d.pixels, d.width, d.height), 0, 0);
  }
</script>

<script lang="ts">
  import { type Frame, type Rect, lineBox, sheetTransform } from "./layout";
  import {
    type Drawn,
    Dropped,
    type FieldInfo,
    type Hit,
    type PageLink,
    type PageSize,
    type Pdf,
    Priority,
    type Target,
    type TextLine,
  } from "./pdf";
  import { type ReadingMode, paperCss } from "./recolor";

  let {
    pdf,
    index,
    size,
    frame,
    scale,
    rotation,
    dpr,
    mode,
    clip,
    hits = [],
    currentHit = -1,
    version,
    onlink,
    onfill,
  }: {
    pdf: Pdf;
    index: number;
    /** Unrotated, in points. */
    size: PageSize;
    frame: Frame;
    /** CSS pixels per point. */
    scale: number;
    rotation: number;
    dpr: number;
    mode: ReadingMode;
    /** The part of the page in view, in page space; null when none is. */
    clip: Rect | null;
    hits?: Hit[];
    currentHit?: number;
    /** Goes up whenever an edit changes the page. */
    version: number;
    onlink: (target: Target) => void;
    /** A field the reader filled in: its new value, or none to toggle a check box. */
    onfill: (field: FieldInfo, value?: string) => void;
  } = $props();

  let host: HTMLDivElement | undefined = $state();
  let base: HTMLCanvasElement | undefined = $state();
  let detail: HTMLCanvasElement | undefined = $state();
  /** Within a screen of the view: drawn while near, let go when far. */
  let near = $state(false);
  let lines: TextLine[] | null = $state(null);
  let links: PageLink[] | null = $state(null);
  let fields: FieldInfo[] | null = $state(null);
  /** The sharp tile on the detail canvas: device pixels at `res` per point. */
  let tile: { x: number; y: number; width: number; height: number; res: number; key: string } | null =
    $state(null);

  const width = $derived(size.width * scale);
  const height = $derived(size.height * scale);
  const res = $derived(scale * dpr);
  const baseRes = $derived(Math.min(res, Math.sqrt(MAX_PIXELS / (size.width * size.height))));
  /** The whole-page canvas is coarser than the screen, so a tile draws the view sharply. */
  const sharp = $derived(baseRes < res * 0.999);
  const look = $derived(`${mode}|${version}`);

  let baseDrawn = "";
  let baseBusy = false;
  let detailBusy = false;
  let detailTimer = 0;
  let asked = { text: false, links: false };
  let fieldsAt = -1;

  $effect(() => {
    if (!host) return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        near = entry.isIntersecting;
        if (near) return;
        if (base) base.width = 0;
        if (detail) detail.width = 0;
        baseDrawn = "";
        tile = null;
      },
      { rootMargin: "100% 0px" },
    );
    observer.observe(host);
    return () => observer.disconnect();
  });

  $effect(() => {
    if (near && base && `${baseRes}|${look}` !== baseDrawn) void drawBase();
  });

  async function drawBase(): Promise<void> {
    if (baseBusy || !base) return;
    baseBusy = true;
    const [r, m, k] = [baseRes, mode, look];
    try {
      // Unmounted pages lose their canvas; their requests are dropped.
      const drawn = await pdf.render(index, r, m, undefined, Priority.Visible, () => near && !!base);
      if (near && base) {
        paint(base, drawn);
        baseDrawn = `${r}|${k}`;
      }
    } catch (e) {
      if (!(e instanceof Dropped)) console.warn(`page ${index + 1}:`, e);
    } finally {
      baseBusy = false;
    }
    // The zoom, the reading mode or the page may have changed while this one was drawn. An
    // unmounted page has no canvas, and its derived values are not to be read.
    if (base && near && `${baseRes}|${look}` !== baseDrawn) void drawBase();
  }

  /** The page-space area a new tile should cover, or null when the current one does. */
  function wantedTile(): Rect | null {
    if (!sharp || !near || !clip) return null;
    if (tile && tile.res === res && tile.key === look) {
      const [x0, y0] = [tile.x / tile.res, tile.y / tile.res];
      const [x1, y1] = [(tile.x + tile.width) / tile.res, (tile.y + tile.height) / tile.res];
      if (x0 <= clip.x0 && y0 <= clip.y0 && x1 >= clip.x1 && y1 >= clip.y1) return null;
    }
    // Half the view more on each side, so small scrolls need no new tile.
    const [dx, dy] = [(clip.x1 - clip.x0) / 2, (clip.y1 - clip.y0) / 2];
    return {
      x0: Math.max(clip.x0 - dx, 0),
      y0: Math.max(clip.y0 - dy, 0),
      x1: Math.min(clip.x1 + dx, size.width),
      y1: Math.min(clip.y1 + dy, size.height),
    };
  }

  $effect(() => {
    if (!sharp && tile) tile = null;
    if (!wantedTile()) return;
    clearTimeout(detailTimer);
    detailTimer = setTimeout(() => void drawDetail(), 100);
    return () => clearTimeout(detailTimer);
  });

  async function drawDetail(): Promise<void> {
    const region = wantedTile();
    if (detailBusy || !detail || !region) return;
    detailBusy = true;
    const [r, m, k] = [res, mode, look];
    try {
      const drawn = await pdf.render(index, r, m, region, Priority.Visible, () => !!detail && near && sharp);
      if (detail && near && sharp) {
        paint(detail, drawn);
        tile = { x: drawn.x, y: drawn.y, width: drawn.width, height: drawn.height, res: r, key: k };
      }
    } catch (e) {
      if (!(e instanceof Dropped)) console.warn(`page ${index + 1}:`, e);
    } finally {
      detailBusy = false;
    }
    if (detail && wantedTile()) void drawDetail();
  }

  $effect(() => {
    if (!near) return;
    if (!lines && !asked.text) {
      asked.text = true;
      pdf.text(index, () => near && !!host).then(
        (l) => (lines = l),
        (e) => (e instanceof Dropped ? (asked.text = false) : (lines = [])),
      );
    }
    if (!links && !asked.links) {
      asked.links = true;
      pdf.links(index, () => near && !!host).then(
        (l) => (links = l),
        (e) => (e instanceof Dropped ? (asked.links = false) : (links = [])),
      );
    }
  });

  // Fields change with edits: a calculation can fill in one from another.
  $effect(() => {
    if (!near || fieldsAt === version) return;
    const v = (fieldsAt = version);
    pdf.fields(index, () => near && !!host).then(
      (f) => fieldsAt === v && (fields = f),
      (e) => (e instanceof Dropped ? (fieldsAt = -1) : (fields = [])),
    );
  });

  const fieldFont = (r: Rect) => Math.min((r.y1 - r.y0) * 0.7, 12) * scale;

  function box(r: Rect): string {
    const [x, y] = [r.x0 * scale, r.y0 * scale];
    return `left:${x}px;top:${y}px;width:${(r.x1 - r.x0) * scale}px;height:${(r.y1 - r.y0) * scale}px`;
  }
</script>

<div
  class="page"
  bind:this={host}
  role="group"
  aria-label="Page {index + 1}"
  data-page={index}
  style:left="{frame.x}px"
  style:top="{frame.y}px"
  style:width="{frame.width}px"
  style:height="{frame.height}px"
  style:background={paperCss(mode)}
>
  <div
    class="sheet"
    style:width="{width}px"
    style:height="{height}px"
    style:transform={sheetTransform(rotation, width, height)}
  >
    <canvas class="base" bind:this={base}></canvas>
    <canvas
      class="detail"
      bind:this={detail}
      hidden={!tile}
      style:left="{tile ? (tile.x / tile.res) * scale : 0}px"
      style:top="{tile ? (tile.y / tile.res) * scale : 0}px"
      style:width="{tile ? (tile.width / tile.res) * scale : 0}px"
      style:height="{tile ? (tile.height / tile.res) * scale : 0}px"
    ></canvas>
    {#if hits.length}
      <div class="hits">
        {#each hits as hit, i (i)}
          {#each hit as r, j (j)}
            <div class="hit" class:current={i === currentHit} style={box(r)}></div>
          {/each}
        {/each}
      </div>
    {/if}
    {#if lines}
      <div class="text">
        {#each lines as line, i (i)}
          {@const b = lineBox(line, scale)}
          <span
            style:left="{b.left}px"
            style:top="{b.top}px"
            style:font-size="{b.fontSize}px"
            style:line-height="{b.height}px"
            style:transform="scaleX({stretch(line.text, b.width, b.fontSize)})">{line.text}</span
          >
        {/each}
      </div>
    {/if}
    {#if links}
      {#each links as link, i (i)}
        {#if link.uri}
          <a
            class="link"
            href={link.uri}
            target="_blank"
            rel="noopener noreferrer"
            title={link.uri}
            aria-label={link.uri}
            style={box(link.rect)}
          ></a>
        {:else if link.page !== undefined}
          <button
            class="link"
            title="Page {link.page + 1}"
            aria-label="Go to page {link.page + 1}"
            style={box(link.rect)}
            onclick={() => onlink(link)}
          ></button>
        {/if}
      {/each}
    {/if}
    {#if fields}
      {#each fields as f (f.id)}
        {#if f.readOnly || f.type === "button" || f.type === "signature"}
          <!-- Nothing to fill in. -->
        {:else if f.type === "checkbox" || f.type === "radiobutton"}
          <button
            class="field toggle"
            role={f.type === "checkbox" ? "checkbox" : "radio"}
            aria-checked={f.checked}
            aria-label={f.name}
            style={box(f.rect)}
            onclick={() => onfill(f)}
          ></button>
        {:else if f.type === "text" && f.multiline}
          <textarea
            class="field"
            aria-label={f.name}
            style={box(f.rect)}
            style:font-size="{fieldFont(f.rect)}px"
            maxlength={f.maxLen || undefined}
            value={f.value}
            onchange={(e) => onfill(f, e.currentTarget.value)}
          ></textarea>
        {:else if f.type === "text"}
          <input
            class="field"
            aria-label={f.name}
            style={box(f.rect)}
            style:font-size="{fieldFont(f.rect)}px"
            maxlength={f.maxLen || undefined}
            value={f.value}
            onchange={(e) => onfill(f, e.currentTarget.value)}
          />
        {:else}
          <select
            class="field"
            aria-label={f.name}
            style={box(f.rect)}
            style:font-size="{fieldFont(f.rect)}px"
            onchange={(e) => onfill(f, e.currentTarget.value)}
          >
            {#each f.options as option (option)}
              <option selected={option === f.value}>{option}</option>
            {/each}
          </select>
        {/if}
      {/each}
    {/if}
  </div>
</div>

<style>
  .page {
    position: absolute;
    overflow: hidden;
    box-shadow: 0 var(--sheet-shadow-offset) var(--sheet-shadow-blur) var(--color-shadow-sheet);
  }

  .sheet {
    position: absolute;
    left: 0;
    top: 0;
    transform-origin: 0 0;
  }

  canvas {
    position: absolute;
  }

  .base {
    inset: 0;
    width: 100%;
    height: 100%;
  }

  .hits,
  .text {
    position: absolute;
    inset: 0;
  }

  .hit {
    position: absolute;
    background: color-mix(in srgb, var(--color-warning) 40%, transparent);
  }

  .hit.current {
    background: color-mix(in srgb, var(--color-accent) 55%, transparent);
    outline: 2px solid var(--color-accent);
  }

  .text {
    overflow: hidden;
    line-height: 1;
  }

  .text span {
    position: absolute;
    white-space: pre;
    color: transparent;
    font-family: sans-serif;
    transform-origin: 0 0;
    cursor: text;
  }

  .text span::selection {
    background: color-mix(in srgb, var(--color-accent) 35%, transparent);
  }

  .link {
    position: absolute;
    display: block;
    padding: 0;
    border: 0;
    background: transparent;
    cursor: pointer;
  }

  /* The page draws the field's value; the control shows only while it is being used. */
  .field {
    position: absolute;
    box-sizing: border-box;
    margin: 0;
    padding: 0 2px;
    border: 1px solid transparent;
    border-radius: 0;
    background: color-mix(in srgb, var(--color-accent) 8%, transparent);
    color: transparent;
    font-family: Helvetica, Arial, sans-serif;
    resize: none;
    appearance: none;
    cursor: pointer;
  }

  input.field,
  textarea.field {
    cursor: text;
  }

  .field:hover {
    border-color: color-mix(in srgb, var(--color-accent) 50%, transparent);
  }

  .field:focus {
    outline: 2px solid var(--color-accent);
  }

  input.field:focus,
  textarea.field:focus,
  select.field:focus {
    background: #fff;
    color: #000;
  }

  option {
    color: #000;
  }

  .link:hover,
  .link:focus-visible {
    background: color-mix(in srgb, var(--color-accent) 15%, transparent);
    outline: 1px solid color-mix(in srgb, var(--color-accent) 50%, transparent);
  }
</style>
