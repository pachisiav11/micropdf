<script lang="ts">
  import { onMount, tick } from "svelte";
  import { type Inline, type Status, type Turn, ask, blocks, loadChat, newChat, status } from "./assistant";

  interface Props {
    /** The PDF's content hash, which names its chat. */
    hash: string;
    /** The page being read, from 0. */
    current: number;
    /** Each page's text, read once when first needed. */
    pages: () => Promise<string[]>;
    /** Shows a cited page, from 0. */
    ongo: (page: number) => void;
    onclose: () => void;
  }

  let { hash, current, pages, ongo, onclose }: Props = $props();

  let setup: Status | null = $state(null);
  /** Why the bridge could not be reached. */
  let unreachable = $state("");
  let turns: Turn[] = $state.raw([]);
  let draft = $state("");
  let busy = $state(false);
  let error = $state("");
  let copied = $state(-1);
  let thread: HTMLElement | undefined = $state();
  let input: HTMLTextAreaElement | undefined = $state();

  const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));

  onMount(() => {
    void Promise.all([status(), loadChat(hash)]).then(
      ([s, t]) => ((setup = s), (turns = t)),
      (e) => (unreachable = reason(e)),
    );
    input?.focus();
  });

  $effect(() => {
    void [turns.length, busy, error];
    void tick().then(() => thread?.scrollTo({ top: thread.scrollHeight }));
  });

  async function send(prompt: string, focus: number | undefined, pageOnly = false): Promise<void> {
    prompt = prompt.trim();
    if (busy || !prompt) return;
    const before = turns;
    turns = [...turns, { role: "user", text: prompt }];
    draft = "";
    error = "";
    busy = true;
    try {
      const answer = await ask({ hash, prompt, pages: await pages(), focus, pageOnly });
      turns = [...turns, answer.turn];
      if (setup) setup.usage = answer.usage;
    } catch (e) {
      turns = before;
      draft = prompt;
      error = reason(e);
    } finally {
      busy = false;
    }
  }

  /** One of the suggestions: the document, this page or the selected text. */
  export function quick(what: "document" | "page" | "selection"): void {
    if (what === "document") {
      void send("Summarize this document in a few short paragraphs or bullet points.", undefined);
    } else if (what === "page") {
      void send(`Summarize page ${current + 1}.`, current, true);
    } else {
      const text = getSelection()?.toString().trim();
      if (text) void send(`Explain this passage:\n\n${text}`, current);
      else error = "Select some text on a page first.";
    }
  }

  async function clear(): Promise<void> {
    try {
      await newChat(hash);
      turns = [];
      error = "";
      input?.focus();
    } catch (e) {
      error = reason(e);
    }
  }

  async function copy(i: number): Promise<void> {
    await navigator.clipboard.writeText(turns[i].text);
    copied = i;
    setTimeout(() => copied === i && (copied = -1), 1500);
  }

  function key(e: KeyboardEvent): void {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void send(draft, current);
    }
  }
</script>

{#snippet line(parts: Inline[])}
  {#each parts as part, i (i)}
    {#if part.page}
      <button class="cite" title="Go to page {part.page}" onclick={() => ongo(part.page! - 1)}>{part.text}</button>
    {:else if part.href}
      <a href={part.href} target="_blank" rel="noreferrer">{part.text}</a>
    {:else if part.code}
      <code>{part.text}</code>
    {:else}
      <span class:b={part.strong} class:i={part.em}>{part.text}</span>
    {/if}
  {/each}
{/snippet}

<aside class="assistant" aria-label="Assistant">
  <header>
    <span class="title" title={setup?.model}>{setup?.model || "Assistant"}</span>
    <button class="icon" aria-label="New chat" title="New chat" disabled={busy || !turns.length} onclick={clear}>
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3v10M3 8h10" /></svg>
    </button>
    <button class="icon" aria-label="Close assistant" title="Close (Ctrl+Shift+A)" onclick={onclose}>×</button>
  </header>

  <div class="thread" bind:this={thread} aria-live="polite">
    {#if unreachable}
      <p class="note">
        The assistant works through micropdf for Windows, which keeps the API key. It could not be reached:
        {unreachable}.
      </p>
    {:else if setup?.notice}
      <p class="note">
        {setup.notice} Set it up in micropdf for Windows: open the assistant (Ctrl+Shift+A) and press its settings button.
      </p>
    {/if}
    {#if !turns.length && !busy}
      <div class="empty">
        <p>Ask about this PDF. Answers cite the pages they come from.</p>
        <!-- Pressing a suggestion keeps the text selection. -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="suggestions" onmousedown={(e) => e.preventDefault()}>
          <button onclick={() => quick("document")}>Summarize the document</button>
          <button onclick={() => quick("page")}>Summarize this page</button>
          <button onclick={() => quick("selection")}>Explain the selection</button>
        </div>
      </div>
    {/if}
    {#each turns as turn, i (i)}
      {#if turn.role === "user"}
        <p class="question">{turn.text}</p>
      {:else}
        <div class="answer">
          {#each blocks(turn.text) as block, b (b)}
            {#if block.kind === "pre"}
              <pre>{block.text}</pre>
            {:else if "items" in block}
              <svelte:element this={block.kind}>
                {#each block.items as item, j (j)}<li>{@render line(item)}</li>{/each}
              </svelte:element>
            {:else}
              <p class:b={block.kind === "h"}>{@render line(block.inlines)}</p>
            {/if}
          {/each}
          <div class="meta">
            <span>{turn.note}</span>
            <button class="text" onclick={() => copy(i)}>{copied === i ? "Copied" : "Copy"}</button>
          </div>
        </div>
      {/if}
    {/each}
    {#if busy}
      <p class="busy">Reading the document…</p>
    {/if}
    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}
  </div>

  <footer>
    <textarea
      bind:this={input}
      bind:value={draft}
      rows="3"
      aria-label="Question"
      placeholder="Ask about this PDF (Ctrl+Enter)"
      onkeydown={key}
    ></textarea>
    <div class="row">
      <span class="usage">{setup?.usage}</span>
      <button class="text primary" disabled={busy || !draft.trim()} onclick={() => send(draft, current)}>Ask</button>
    </div>
  </footer>
</aside>

<style>
  .assistant {
    display: flex;
    flex: none;
    flex-direction: column;
    width: var(--assistant-width);
    max-width: 50vw;
    border-left: 1px solid var(--color-border-muted);
    background: var(--color-bg-secondary);
  }

  header {
    display: flex;
    align-items: center;
    gap: 4px;
    height: var(--toolbar-height);
    padding: 0 8px 0 12px;
    border-bottom: 1px solid var(--color-border-muted);
  }

  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--color-text-muted);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  button {
    height: 28px;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--color-text);
    font: inherit;
    cursor: pointer;
  }

  button:hover:not(:disabled) {
    background: var(--color-bg-hover);
  }

  button:disabled {
    color: var(--color-text-subtle);
    cursor: default;
  }

  .icon {
    display: grid;
    place-items: center;
    width: 28px;
    padding: 0;
  }

  .text {
    padding: 0 10px;
  }

  .primary {
    border-color: var(--color-accent);
    background: var(--color-accent);
    color: var(--color-on-accent);
  }

  .primary:hover:not(:disabled) {
    background: var(--color-accent-hover);
  }

  .primary:disabled {
    opacity: 0.5;
    color: var(--color-on-accent);
  }

  svg {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.3;
    stroke-linecap: round;
  }

  .thread {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 12px;
    line-height: 1.5;
    overflow-wrap: anywhere;
  }

  .thread p {
    margin: 0 0 8px;
  }

  .note,
  .error {
    padding: 8px 10px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
  }

  .error {
    border-color: var(--color-danger);
    color: var(--color-danger);
  }

  .empty {
    margin-top: 16px;
    color: var(--color-text-muted);
  }

  .suggestions {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    margin-top: 12px;
  }

  .suggestions button {
    padding: 0 10px;
    border-color: var(--color-border);
    border-radius: 14px;
  }

  .question {
    margin: 12px 0 8px 32px !important;
    padding: 8px 10px;
    border-radius: var(--radius);
    background: var(--color-bg-active);
    white-space: pre-wrap;
  }

  .answer {
    margin-bottom: 12px;
  }

  .answer ul,
  .answer ol {
    margin: 0 0 8px;
    padding-left: 20px;
  }

  .b {
    font-weight: 600;
  }

  .i {
    font-style: italic;
  }

  code,
  pre {
    font-family: var(--font-mono);
    font-size: 0.92em;
  }

  pre {
    margin: 0 0 8px;
    padding: 8px;
    overflow-x: auto;
    border-radius: var(--radius-sm);
    background: var(--color-bg-tertiary);
  }

  .cite {
    height: auto;
    padding: 0 3px;
    border-radius: var(--radius-sm);
    background: var(--color-accent-glow);
    color: var(--color-text-link);
    font-size: 0.9em;
  }

  a {
    color: var(--color-text-link);
  }

  .meta {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--color-text-subtle);
    font-size: 12px;
  }

  .meta span {
    flex: 1;
  }

  .meta button {
    height: 22px;
    color: var(--color-text-muted);
    font-size: 12px;
  }

  .busy {
    color: var(--color-text-muted);
  }

  footer {
    padding: 8px 12px 12px;
    border-top: 1px solid var(--color-border-muted);
  }

  textarea {
    box-sizing: border-box;
    width: 100%;
    padding: 6px 8px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    background: var(--color-bg-tertiary);
    color: var(--color-text);
    font: inherit;
    resize: none;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 6px;
  }

  .usage {
    flex: 1;
    color: var(--color-text-subtle);
    font-size: 12px;
  }
</style>
