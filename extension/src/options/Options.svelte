<script lang="ts">
  import { onMount } from "svelte";
  import { DEFAULTS, type Settings, loadSettings, saveSettings } from "../settings";

  let settings: Settings = $state({ ...DEFAULTS });
  /** Whether the reader let the extension read file:// URLs; null until known. */
  let fileAccess: boolean | null = $state(null);

  onMount(async () => {
    settings = await loadSettings();
    fileAccess = await chrome.extension.isAllowedFileSchemeAccess();
  });

  function save(change: Partial<Settings>): void {
    settings = { ...settings, ...change };
    void saveSettings(change);
  }

  const OPEN: [Settings["open"], string, string][] = [
    ["viewer", "Show it in micropdf's viewer", "Comment, fill in forms and save in the browser."],
    ["app", "Open it in micropdf for Windows", "Needs the desktop app; the browser tab goes back."],
    ["browser", "Leave it to the browser", "The browser's own viewer shows it."],
  ];
</script>

<main>
  <h1>micropdf</h1>
  <fieldset>
    <legend>When the browser opens a PDF</legend>
    {#each OPEN as [value, label, note] (value)}
      <label class="choice">
        <input type="radio" name="open" {value} checked={settings.open === value} onchange={() => save({ open: value })} />
        <span>{label}<small>{note}</small></span>
      </label>
    {/each}
  </fieldset>
  <label class="field">
    Your name on comments
    <input
      value={settings.author}
      placeholder="Anonymous"
      onchange={(e) => save({ author: e.currentTarget.value.trim() })}
    />
  </label>
  {#if fileAccess === false}
    <p class="note">
      To open PDFs from this computer here, turn on “Allow access to file URLs” for micropdf on the
      browser's extensions page.
    </p>
  {/if}
</main>

<style>
  :global(body) {
    margin: 0;
    background: var(--color-bg);
    color: var(--color-text);
    font: 14px/1.4 var(--font-ui);
  }

  main {
    display: flex;
    flex-direction: column;
    gap: 16px;
    max-width: 520px;
    padding: 16px 20px 24px;
  }

  h1 {
    margin: 0;
    font: 600 18px var(--font-display);
  }

  fieldset {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0;
    padding: 0;
    border: 0;
  }

  legend {
    margin-bottom: 8px;
    font-weight: 600;
  }

  .choice {
    display: flex;
    align-items: flex-start;
    gap: 8px;
  }

  small {
    display: block;
    color: var(--color-text-muted);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-weight: 600;
  }

  .field input {
    padding: 6px 8px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    background: var(--color-bg-tertiary);
    color: var(--color-text);
    font: 14px var(--font-ui);
  }

  .note {
    margin: 0;
    color: var(--color-text-muted);
  }
</style>
