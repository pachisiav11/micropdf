<script lang="ts">
  import type { DrawTool } from "./annotate";
  import { type Layout, frameToPage, hit, pageToFrame } from "./layout";
  import type { Point } from "./pdf";

  let {
    layout,
    tool,
    color,
    ondraw,
  }: {
    layout: Layout;
    tool: DrawTool;
    /** CSS colour of the preview. */
    color: string;
    /** A finished press or drag, in page space. */
    ondraw: (page: number, points: Point[]) => void;
  } = $props();

  let surface: HTMLDivElement | undefined = $state();
  let drag: { page: number; points: Point[] } | null = $state.raw(null);

  function local(e: PointerEvent): [number, number] {
    const r = surface!.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  }

  function down(e: PointerEvent): void {
    if (e.button !== 0) return;
    const at = hit(layout, ...local(e));
    if (!at) return;
    e.preventDefault();
    surface?.setPointerCapture(e.pointerId);
    drag = { page: at[0], points: [[at[1], at[2]]] };
  }

  function move(e: PointerEvent): void {
    if (!drag) return;
    const { page, points } = drag;
    const f = layout.frames[page]!;
    const [x, y] = local(e);
    // Held to the page the drag started on.
    const fx = Math.min(Math.max(x - f.x, 0), f.width);
    const fy = Math.min(Math.max(y - f.y, 0), f.height);
    const p = frameToPage(layout, page, fx, fy);
    if (tool !== "ink") drag = { page, points: [points[0], p] };
    else if (Math.hypot(p[0] - points.at(-1)![0], p[1] - points.at(-1)![1]) >= 1) drag = { page, points: [...points, p] };
  }

  function up(): void {
    if (!drag) return;
    const { page, points } = drag;
    drag = null;
    ondraw(page, points);
  }

  /** The drag in surface pixels. */
  const shown = $derived.by(() => {
    if (!drag) return [];
    const f = layout.frames[drag.page]!;
    return drag.points.map(([x, y]) => {
      const [fx, fy] = pageToFrame(layout, drag!.page, x, y);
      return [f.x + fx, f.y + fy];
    });
  });
  const [a, b] = $derived([shown[0], shown.at(-1)]);
</script>

<div
  class="draw"
  bind:this={surface}
  role="application"
  aria-label="Drawing surface"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={() => (drag = null)}
>
  {#if a && b}
    <svg stroke={color} aria-hidden="true">
      {#if tool === "ink"}
        <polyline points={shown.map((p) => p.join(",")).join(" ")} stroke-width="2" />
      {:else if tool === "line" || tool === "arrow"}
        <line x1={a[0]} y1={a[1]} x2={b[0]} y2={b[1]} stroke-width="1.5" />
      {:else if tool === "circle"}
        <ellipse
          cx={(a[0] + b[0]) / 2}
          cy={(a[1] + b[1]) / 2}
          rx={Math.abs(b[0] - a[0]) / 2}
          ry={Math.abs(b[1] - a[1]) / 2}
          stroke-width="1.5"
        />
      {:else if tool !== "note"}
        <rect
          x={Math.min(a[0], b[0])}
          y={Math.min(a[1], b[1])}
          width={Math.abs(b[0] - a[0])}
          height={Math.abs(b[1] - a[1])}
          stroke-width="1.5"
          stroke-dasharray={tool === "text" ? "4 3" : undefined}
        />
      {/if}
    </svg>
  {/if}
</div>

<style>
  .draw {
    position: absolute;
    inset: 0;
    cursor: crosshair;
    touch-action: none;
  }

  svg {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    fill: none;
    stroke-linecap: round;
    stroke-linejoin: round;
    pointer-events: none;
  }
</style>
