// The options page's settings, kept in chrome.storage.sync so they follow the reader's profile.

export interface Settings {
  /** The name new comments are by. */
  author: string;
  /** What happens to a PDF the browser opens: micropdf's viewer, the desktop app, or the
   * browser's own viewer. */
  open: "viewer" | "app" | "browser";
}

export const DEFAULTS: Settings = { author: "", open: "viewer" };

export async function loadSettings(): Promise<Settings> {
  try {
    return { ...DEFAULTS, ...(await chrome.storage.sync.get(DEFAULTS)) } as Settings;
  } catch {
    // Outside the extension, as on the dev server.
    return DEFAULTS;
  }
}

export function saveSettings(change: Partial<Settings>): Promise<void> {
  return chrome.storage.sync.set(change);
}
