<script lang="ts">
  import type { OutlineNode, PageSize, Pdf, Target } from "./pdf";
  import type { ReadingMode } from "./recolor";
  import Thumb from "./Thumb.svelte";

  let {
    pdf,
    pages,
    rotation,
    dpr,
    mode,
    current,
    outline,
    tab = $bindable(),
    ongo,
  }: {
    pdf: Pdf;
    pages: PageSize[];
    rotation: number;
    dpr: number;
    mode: ReadingMode;
    current: number;
    outline: OutlineNode[];
    tab: "pages" | "outline";
    ongo: (target: Target) => void;
  } = $props();

  /** Bookmarks the reader opened or closed, over the PDF's own open state. */
  let toggled = $state(new Set<OutlineNode>());

  const isOpen = (node: OutlineNode) => node.open !== toggled.has(node);

  function toggle(node: OutlineNode): void {
    const next = new Set(toggled);
    if (!next.delete(node)) next.add(node);
    toggled = next;
  }

  const shown = $derived(outline.length ? tab : "pages");
</script>

{#snippet tree(nodes: OutlineNode[], depth: number)}
  <ul role={depth ? "group" : "tree"} aria-label={depth ? undefined : "Bookmarks"}>
    {#each nodes as node, i (i)}
      <li role="treeitem" aria-selected="false" aria-expanded={node.children.length ? isOpen(node) : undefined}>
        <div class="row" style:padding-left="{depth * 14}px">
          {#if node.children.length}
            <button class="twisty" aria-label={isOpen(node) ? "Collapse" : "Expand"} onclick={() => toggle(node)}
              >{isOpen(node) ? "▾" : "▸"}</button
            >
          {:else}
            <span class="twisty"></span>
          {/if}
          <button
            class="title"
            disabled={node.page === undefined && !node.uri}
            title={node.uri ?? node.title}
            onclick={() => ongo(node)}>{node.title || "Untitled"}</button
          >
        </div>
        {#if node.children.length && isOpen(node)}
          {@render tree(node.children, depth + 1)}
        {/if}
      </li>
    {/each}
  </ul>
{/snippet}

<aside class="sidebar">
  {#if outline.length}
    <div class="tabs" role="tablist">
      <button role="tab" aria-selected={shown === "pages"} onclick={() => (tab = "pages")}>Pages</button>
      <button role="tab" aria-selected={shown === "outline"} onclick={() => (tab = "outline")}>Bookmarks</button>
    </div>
  {/if}
  <div class="list" role="tabpanel">
    {#if shown === "pages"}
      {#each pages as size, i (i)}
        <Thumb
          {pdf}
          index={i}
          {size}
          {rotation}
          {dpr}
          {mode}
          current={i === current}
          onpick={(page) => ongo({ page })}
        />
      {/each}
    {:else}
      {@render tree(outline, 0)}
    {/if}
  </div>
</aside>

<style>
  .sidebar {
    display: grid;
    grid-template-rows: auto 1fr;
    min-height: 0;
    width: var(--sidebar-width);
    background: var(--color-bg-secondary);
    border-right: 1px solid var(--color-border-muted);
  }

  .tabs {
    display: flex;
    gap: 4px;
    padding: 8px 8px 0;
  }

  .tabs button {
    flex: 1;
    padding: 6px 8px;
    border: 0;
    border-bottom: 2px solid transparent;
    background: transparent;
    color: var(--color-text-muted);
    font: inherit;
    cursor: pointer;
  }

  .tabs button[aria-selected="true"] {
    border-bottom-color: var(--color-accent);
    color: var(--color-text);
  }

  .list {
    grid-row: 2;
    overflow: auto;
    padding: 8px;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .row {
    display: flex;
    align-items: baseline;
  }

  .twisty {
    flex: none;
    width: 20px;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--color-text-muted);
    font: inherit;
    cursor: pointer;
  }

  .title {
    flex: 1;
    min-width: 0;
    padding: 4px 6px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    text-align: left;
    overflow-wrap: anywhere;
    cursor: pointer;
  }

  .title:hover:not(:disabled) {
    background: var(--color-bg-hover);
  }

  .title:disabled {
    color: var(--color-text-subtle);
    cursor: default;
  }
</style>
