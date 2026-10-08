// The service worker: sends the PDFs the browser opens to the viewer, unless the reader chose
// otherwise, and hands PDFs to micropdf for Windows from the toolbar button and the link menu.

import { openLocal, sendPdf } from "./bridge";
import { PDF_RULE_ID, fileName, localPath, looksLikePdf, pdfRule } from "./rules";
import { loadSettings } from "./settings";

const VIEWER = chrome.runtime.getURL("viewer.html");
const LINK_MENU = "open-link";

async function installRules(): Promise<void> {
  const { open } = await loadSettings();
  await chrome.declarativeNetRequest.updateDynamicRules({
    removeRuleIds: [PDF_RULE_ID],
    addRules: open === "browser" ? [] : [pdfRule(VIEWER)],
  });
}

chrome.runtime.onInstalled.addListener(() => {
  void installRules();
  chrome.contextMenus.create({ id: LINK_MENU, title: "Open link in micropdf", contexts: ["link"] });
});
chrome.runtime.onStartup.addListener(() => void installRules());
chrome.storage.onChanged.addListener((changes) => changes.open && void installRules());

// Rules cannot redirect file:// pages. The viewer reads them only if the reader turned on file
// access for the extension.
chrome.webNavigation.onBeforeNavigate.addListener(
  async ({ tabId, frameId, url }) => {
    if (frameId !== 0) return;
    const [{ open }, allowed] = await Promise.all([loadSettings(), chrome.extension.isAllowedFileSchemeAccess()]);
    if (open !== "browser" && allowed) await chrome.tabs.update(tabId, { url: `${VIEWER}#${url}` });
  },
  { url: [{ schemes: ["file"], pathSuffix: ".pdf" }, { schemes: ["file"], pathSuffix: ".PDF" }] },
);

/** Shows a failed hand-off on the toolbar button, whose tooltip says why. */
async function report(work: Promise<unknown>): Promise<void> {
  try {
    await work;
    await chrome.action.setBadgeText({ text: "" });
    await chrome.action.setTitle({ title: "Open in micropdf" });
  } catch (e) {
    await chrome.action.setBadgeBackgroundColor({ color: "#d93025" });
    await chrome.action.setBadgeText({ text: "!" });
    await chrome.action.setTitle({ title: `Could not open in micropdf: ${e instanceof Error ? e.message : e}` });
  }
}

async function openUrl(url: string): Promise<void> {
  const path = localPath(url);
  if (path) {
    await openLocal(path);
    return;
  }
  const response = await fetch(url, { credentials: "include" });
  if (!response.ok) throw new Error(`the server answered ${response.status}`);
  await sendPdf(new Uint8Array(await response.arrayBuffer()), { name: fileName(url) });
}

/** Prints the page to a PDF through the debugger, the one way an extension can, and hands that
 * on. The debugger permission is optional, asked for on first use. */
async function openPage(tab: chrome.tabs.Tab, granted: Promise<boolean>): Promise<void> {
  if (!(await granted)) return;
  const target = { tabId: tab.id! };
  await chrome.debugger.attach(target, "1.3");
  try {
    const { data } = (await chrome.debugger.sendCommand(target, "Page.printToPDF", { printBackground: true })) as {
      data: string;
    };
    const bytes = Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
    await sendPdf(bytes, { name: `${tab.title || "page"}.pdf` });
  } finally {
    await chrome.debugger.detach(target).catch(() => {});
  }
}

chrome.action.onClicked.addListener((tab) => {
  if (!tab.id || !tab.url) return;
  if (tab.url.startsWith(VIEWER)) {
    void chrome.tabs.sendMessage(tab.id, { type: "open-in-app" });
  } else if (looksLikePdf(tab.url)) {
    void report(openUrl(tab.url));
  } else {
    // The permission prompt needs the click's user gesture, so it comes before anything else.
    void report(openPage(tab, chrome.permissions.request({ permissions: ["debugger"] })));
  }
});

chrome.contextMenus.onClicked.addListener((info) => {
  if (info.menuItemId === LINK_MENU && info.linkUrl) void report(openUrl(info.linkUrl));
});
