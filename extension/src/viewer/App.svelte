<script lang="ts">
  import { onMount, tick } from "svelte";
  import { fileName, pdfSource } from "../rules";
  import { PX_PER_POINT, fitWidth, pageAt, pageTops, zoomStep } from "./layout";
  import Page from "./Page.svelte";
  import { Pdf, type Opened, type PageSize } from "./pdf";

  const pdf = new Pdf();

  let name = $state("micropdf");
  let pages: PageSize[] = $state([]);
  /** Shown instead of the pages while loading or after a failure. */
  let message = $state("Loading…");
  let locked = $state(false);
  let password = $state("");
  let zoom = $state(1);
  let fit = $state(true);
  let view: HTMLElement | undefined = $state();
  // The content box, unlike clientWidth's border box, shrinks when a scrollbar appears.
  let box: DOMRectReadOnly | undefined = $state();
  const viewWidth = $derived(box?.width ?? 0);
  const viewHeight = $derived(box?.height ?? 0);
  let scrollTop = $state(0);

  const shownZoom = $derived(fit && pages.length ? fitWidth(pages, viewWidth) : zoom);
  const scale = $derived(shownZoom * PX_PER_POINT);
  const layout = $derived(pageTops(pages, scale));
  const current = $derived(pages.length ? pageAt(layout.tops, scrollTop, viewHeight) : 0);

  function show(opened: Opened): void {
    locked = opened.needsPassword;
    pages = opened.pages;
    message = locked || pages.length ? "" : "The PDF has no pages.";
    if (opened.title.trim()) document.title = opened.title.trim();
  }

  onMount(async () => {
    const src = pdfSource(location.href);
    if (!src) {
      message = "Open a link to a PDF to view it here.";
      return;
    }
    name = fileName(src);
    document.title = name;
    try {
      const response = await fetch(src, { credentials: "include" });
      if (!response.ok) throw new Error(`the server answered ${response.status}`);
      show(await pdf.open(await response.arrayBuffer()));
    } catch (e) {
      message = `Could not open the PDF: ${e instanceof Error ? e.message : e}`;
    }
  });

  async function unlock(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    const opened = await pdf.unlock(password);
    if (opened) show(opened);
    else message = "That password is not right.";
  }

  /** Changes the zoom, keeping the spot at the top of the view, and the middle across, in place. */
  async function rezoom(change: () => void): Promise<void> {
    if (!view) return change();
    const page = current;
    const into = (view.scrollTop - layout.tops[page]) / scale;
    const across = (view.scrollLeft + view.clientWidth / 2) / view.scrollWidth;
    change();
    await tick();
    view.scrollTop = layout.tops[page] + into * scale;
    view.scrollLeft = across * view.scrollWidth - view.clientWidth / 2;
  }

  const zoomTo = (next: number) =>
    rezoom(() => {
      fit = false;
      zoom = next;
    });
  const fitToWidth = () => rezoom(() => (fit = true));

  function keydown(e: KeyboardEvent): void {
    if (!(e.ctrlKey || e.metaKey) || !pages.length) return;
    if (e.key === "=" || e.key === "+") void zoomTo(zoomStep(shownZoom, 1));
    else if (e.key === "-") void zoomTo(zoomStep(shownZoom, -1));
    else if (e.key === "0") void fitToWidth();
    else return;
    e.preventDefault();
  }

  // Ctrl+wheel zooms the document, not the browser tab; that needs a listener that is not passive.
  $effect(() => {
    if (!view) return;
    const wheel = (e: WheelEvent) => {
      if (!e.ctrlKey || !pages.length) return;
      e.preventDefault();
      void zoomTo(zoomStep(shownZoom, e.deltaY < 0 ? 1 : -1));
    };
    view.addEventListener("wheel", wheel, { passive: false });
    return () => view?.removeEventListener("wheel", wheel);
  });
</script>

<!-- The hash names the PDF, which is read once; a new one means a new document. -->
<svelte:window onkeydown={keydown} onhashchange={() => location.reload()} />

<header class="toolbar">
  <span class="name" title={name}>{name}</span>
  {#if pages.length}
    <span class="pages" aria-live="polite">{current + 1} / {pages.length}</span>
    <div class="zoom">
      <button
        aria-label="Zoom out"
        title="Zoom out (Ctrl+−)"
        onclick={() => zoomTo(zoomStep(shownZoom, -1))}>−</button
      >
      <span class="percent">{Math.round(shownZoom * 100)}%</span>
      <button aria-label="Zoom in" title="Zoom in (Ctrl+=)" onclick={() => zoomTo(zoomStep(shownZoom, 1))}
        >+</button
      >
      <button class:active={fit} title="Fit width (Ctrl+0)" onclick={fitToWidth}>Fit width</button>
    </div>
  {/if}
</header>

<main
  bind:this={view}
  bind:contentRect={box}
  onscroll={() => (scrollTop = view?.scrollTop ?? 0)}
>
  {#if locked}
    <form class="message" onsubmit={unlock}>
      <label for="password">{message || "This PDF has a password."}</label>
      <input id="password" type="password" bind:value={password} autocomplete="off" />
      <button type="submit">Open</button>
    </form>
  {:else if message}
    <p class="message">{message}</p>
  {:else}
    <div class="column" style:width="{layout.width}px" style:height="{layout.height}px">
      {#each pages as size, i (i)}
        <Page {pdf} index={i} {size} {scale} top={layout.tops[i]} />
      {/each}
    </div>
  {/if}
</main>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 16px;
    height: var(--toolbar-height);
    padding: 0 12px;
    background: var(--color-bg-secondary);
    border-bottom: 1px solid var(--color-border-muted);
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-display);
  }

  .pages,
  .percent {
    color: var(--color-text-muted);
    font-variant-numeric: tabular-nums;
  }

  .percent {
    min-width: 4ch;
    text-align: center;
  }

  .zoom {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  button {
    min-width: 28px;
    height: 28px;
    padding: 0 8px;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    cursor: pointer;
  }

  button:hover {
    background: var(--color-bg-hover);
  }

  button.active {
    border-color: var(--color-border);
    background: var(--color-bg-active);
  }

  main {
    position: relative;
    overflow: auto;
  }

  .column {
    position: relative;
    min-width: 100%;
  }

  .message {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    margin: 20vh auto 0;
    max-width: 40ch;
    color: var(--color-text-muted);
    text-align: center;
  }

  input {
    width: 100%;
    padding: 6px 8px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    background: var(--color-bg-tertiary);
    color: var(--color-text);
    font: inherit;
  }
</style>
