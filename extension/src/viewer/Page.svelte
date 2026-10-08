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
</script>

<script lang="ts">
  import { lineBox } from "./layout";
  import type { PageSize, Pdf, TextLine } from "./pdf";

  let {
    pdf,
    index,
    size,
    scale,
    top,
  }: { pdf: Pdf; index: number; size: PageSize; scale: number; top: number } = $props();

  let host: HTMLDivElement | undefined = $state();
  let canvas: HTMLCanvasElement | undefined = $state();
  /** Within a screen of the view: drawn while near, let go when far. */
  let near = $state(false);
  let lines: TextLine[] | null = $state(null);
  /** The device pixels per point the canvas holds; 0 when empty. */
  let drawnAt = 0;
  let drawing = false;

  $effect(() => {
    if (!host) return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        near = entry.isIntersecting;
        if (!near && canvas) {
          canvas.width = 0;
          drawnAt = 0;
        }
      },
      { rootMargin: "100% 0px" },
    );
    observer.observe(host);
    return () => observer.disconnect();
  });

  $effect(() => {
    const wanted = scale * devicePixelRatio;
    if (near && canvas && wanted !== drawnAt) void draw();
  });

  $effect(() => {
    if (near && !lines) {
      pdf.text(index).then((l) => (lines = l), () => (lines = []));
    }
  });

  async function draw(): Promise<void> {
    if (drawing || !canvas) return;
    drawing = true;
    const wanted = scale * devicePixelRatio;
    try {
      const drawn = await pdf.render(index, wanted);
      if (!near) return;
      canvas.width = drawn.width;
      canvas.height = drawn.height;
      canvas.getContext("2d")?.putImageData(new ImageData(drawn.pixels, drawn.width, drawn.height), 0, 0);
      drawnAt = wanted;
    } finally {
      drawing = false;
    }
    // The zoom may have changed while this one was drawn.
    if (near && scale * devicePixelRatio !== drawnAt) void draw();
  }
</script>

<div
  class="page"
  bind:this={host}
  style:top="{top}px"
  style:width="{size.width * scale}px"
  style:height="{size.height * scale}px"
  role="img"
  aria-label="Page {index + 1}"
>
  <canvas bind:this={canvas}></canvas>
  {#if lines}
    <div class="text">
      {#each lines as line, i (i)}
        {@const box = lineBox(line, scale)}
        <span
          style:left="{box.left}px"
          style:top="{box.top}px"
          style:font-size="{box.fontSize}px"
          style:line-height="{box.height}px"
          style:transform="scaleX({stretch(line.text, box.width, box.fontSize)})">{line.text}</span
        >
      {/each}
    </div>
  {/if}
</div>

<style>
  .page {
    position: absolute;
    left: 50%;
    translate: -50% 0;
    background: #fff;
    box-shadow: 0 var(--sheet-shadow-offset) var(--sheet-shadow-blur) var(--color-shadow-sheet);
  }

  canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
  }

  .text {
    position: absolute;
    inset: 0;
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
</style>
