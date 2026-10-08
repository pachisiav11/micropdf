<script lang="ts">
  import { sheetTransform } from "./layout";
  import { Dropped, type PageSize, type Pdf, Priority } from "./pdf";
  import { type ReadingMode, paperCss } from "./recolor";

  /** The box a thumbnail fits in, in CSS pixels, as on the desktop. */
  const BOX = { width: 140, height: 180 };

  let {
    pdf,
    index,
    size,
    rotation,
    dpr,
    mode,
    current,
    onpick,
  }: {
    pdf: Pdf;
    index: number;
    size: PageSize;
    rotation: number;
    dpr: number;
    mode: ReadingMode;
    current: boolean;
    onpick: (page: number) => void;
  } = $props();

  let host: HTMLButtonElement | undefined = $state();
  let canvas: HTMLCanvasElement | undefined = $state();
  let near = $state(false);
  let drawn = "";
  let busy = false;

  const turned = $derived(rotation % 180 !== 0);
  const scale = $derived(
    Math.min(BOX.width / (turned ? size.height : size.width), BOX.height / (turned ? size.width : size.height)),
  );
  const width = $derived(size.width * scale);
  const height = $derived(size.height * scale);

  $effect(() => {
    if (current) host?.scrollIntoView({ block: "nearest" });
  });

  $effect(() => {
    if (!host) return;
    const observer = new IntersectionObserver(([entry]) => (near = entry.isIntersecting), {
      rootMargin: "50% 0px",
    });
    observer.observe(host);
    return () => observer.disconnect();
  });

  $effect(() => {
    if (near && canvas && `${scale * dpr}|${mode}` !== drawn) void draw();
  });

  async function draw(): Promise<void> {
    if (busy || !canvas) return;
    busy = true;
    const [r, m] = [scale * dpr, mode];
    try {
      const d = await pdf.render(index, r, m, undefined, Priority.Background, () => near && !!canvas);
      if (!canvas) return;
      canvas.width = d.width;
      canvas.height = d.height;
      canvas.getContext("2d")?.putImageData(new ImageData(d.pixels, d.width, d.height), 0, 0);
      drawn = `${r}|${m}`;
    } catch (e) {
      if (!(e instanceof Dropped)) console.warn(`thumbnail ${index + 1}:`, e);
    } finally {
      busy = false;
    }
    if (near && `${scale * dpr}|${mode}` !== drawn) void draw();
  }
</script>

<button
  class="thumb"
  class:current
  bind:this={host}
  aria-label="Page {index + 1}"
  aria-current={current ? "page" : undefined}
  onclick={() => onpick(index)}
>
  <span
    class="frame"
    style:width="{turned ? height : width}px"
    style:height="{turned ? width : height}px"
    style:background={paperCss(mode)}
  >
    <canvas
      bind:this={canvas}
      style:width="{width}px"
      style:height="{height}px"
      style:transform={sheetTransform(rotation, width, height)}
    ></canvas>
  </span>
  <span class="label">{index + 1}</span>
</button>

<style>
  .thumb {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 8px 0;
    border: 0;
    border-radius: var(--radius);
    background: transparent;
    color: var(--color-text-muted);
    font: inherit;
    cursor: pointer;
  }

  .thumb:hover {
    background: var(--color-bg-hover);
  }

  .frame {
    position: relative;
    display: block;
    overflow: hidden;
    box-shadow: 0 1px 3px var(--color-shadow-sheet);
    outline: 2px solid transparent;
    outline-offset: 2px;
  }

  .current .frame {
    outline-color: var(--color-accent);
  }

  .current .label {
    color: var(--color-text);
  }

  canvas {
    position: absolute;
    left: 0;
    top: 0;
    transform-origin: 0 0;
  }

  .label {
    font-variant-numeric: tabular-nums;
  }
</style>
