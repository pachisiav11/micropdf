// The built extension (dist/) in the installed Chrome, and Edge on Windows. Branded browsers ignore
// --load-extension, so it is loaded over the DevTools protocol (Extensions.loadUnpacked), which
// needs --enable-unsafe-extension-debugging and a debugging port. The native bridge is a stand-in
// that reports each message to the test.

import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { type Server, createServer } from "node:http";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { type BrowserContext, type Page, type Worker, test as base, chromium, expect } from "@playwright/test";
import * as mupdf from "mupdf";

const DIST = resolve(import.meta.dirname, "../dist").replaceAll("\\", "/");
const FIXTURES = resolve(import.meta.dirname, "../../fixtures");
const ID = "phhaejfhblmccnkhnhjflbhckkanlnki";
const VIEWER = `chrome-extension://${ID}/viewer.html`;

type BridgeMessage = { type: string; name?: string; target?: string; path?: string; data?: string };

/** Serves the test PDFs as a website would; under /download/ they come as attachments. */
function website(): Promise<Server> {
  const server = createServer((request, response) => {
    const path = new URL(request.url ?? "/", "http://x").pathname;
    const name = path.split("/").pop() ?? "";
    if (!name.endsWith(".pdf")) {
      response.writeHead(200, { "content-type": "text/html" });
      response.end(`<a href="/hello.pdf">hello</a>`);
      return;
    }
    const headers: Record<string, string> = { "content-type": "application/pdf" };
    if (path.startsWith("/download/")) headers["content-disposition"] = `attachment; filename="${name}"`;
    response.writeHead(200, headers);
    response.end(readFileSync(join(FIXTURES, name)));
  });
  return new Promise((done) => server.listen(0, "127.0.0.1", () => done(server)));
}

async function loadUnpacked(profile: string): Promise<void> {
  const port = readFileSync(join(profile, "DevToolsActivePort"), "utf8").split("\n")[0];
  const { webSocketDebuggerUrl } = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
  const socket = new WebSocket(webSocketDebuggerUrl);
  await new Promise((open) => socket.addEventListener("open", open));
  const reply = new Promise<{ error?: { message: string } }>((r) =>
    socket.addEventListener("message", (e) => r(JSON.parse(String(e.data)))),
  );
  socket.send(JSON.stringify({ id: 1, method: "Extensions.loadUnpacked", params: { path: DIST } }));
  const { error } = await reply;
  socket.close();
  if (error) throw new Error(`could not load the extension: ${error.message}`);
}

const test = base.extend<{ context: BrowserContext; worker: Worker; web: string; bridge: BridgeMessage[] }>({
  context: async ({ channel }, use) => {
    const profile = mkdtempSync(join(tmpdir(), "micropdf-e2e-"));
    const context = await chromium.launchPersistentContext(profile, {
      channel,
      ignoreDefaultArgs: ["--disable-extensions"],
      args: ["--enable-unsafe-extension-debugging", "--remote-debugging-port=0"],
    });
    await loadUnpacked(profile);
    await use(context);
    await context.close();
    rmSync(profile, { recursive: true, force: true });
  },
  worker: async ({ context }, use) => {
    const worker = context.serviceWorkers()[0] ?? (await context.waitForEvent("serviceworker"));
    // The rule goes in when the extension installs.
    await expect.poll(() => worker.evaluate(async () => (await chrome.declarativeNetRequest.getDynamicRules()).length)).toBe(1);
    await use(worker);
  },
  bridge: async ({ context }, use) => {
    const sent: BridgeMessage[] = [];
    await context.exposeFunction("bridgeMessage", (m: BridgeMessage) => void sent.push(m));
    await context.addInitScript(() => {
      const runtime = (globalThis as { chrome?: typeof chrome }).chrome?.runtime;
      if (!runtime?.connectNative) return;
      const report = (globalThis as unknown as { bridgeMessage: (m: object) => Promise<void> }).bridgeMessage;
      runtime.connectNative = () => {
        const listeners: ((m: object) => void)[] = [];
        return {
          postMessage(m: { type: string }) {
            void report(m).then(() => {
              const reply =
                m.type === "begin" ? { type: "ready" } : m.type === "chunk" ? { type: "chunk" } : { type: "done", path: "C:\\out.pdf" };
              for (const listener of listeners) listener(reply);
            });
          },
          onMessage: { addListener: (l: (m: object) => void) => listeners.push(l) },
          onDisconnect: { addListener: () => {} },
          disconnect: () => {},
        } as unknown as chrome.runtime.Port;
      };
    });
    await use(sent);
  },
  web: async ({}, use) => {
    const server = await website();
    const { port } = server.address() as { port: number };
    await use(`http://127.0.0.1:${port}`);
    server.close();
  },
  // Asks for the worker so the rule is in before the first page loads.
  page: async ({ context, worker: _ }, use) => use(await context.newPage()),
});

async function drawn(page: Page): Promise<void> {
  const canvas = page.getByRole("group", { name: "Page 1", exact: true }).locator("canvas.base");
  await expect.poll(() => canvas.evaluate((c: HTMLCanvasElement) => c.width)).toBeGreaterThan(200);
}

/** The PDF the bridge received, put together from its chunks. */
function received(sent: BridgeMessage[]): Uint8Array {
  return Buffer.concat(sent.filter((m) => m.type === "chunk").map((m) => Buffer.from(m.data!, "base64")));
}

test("opens web PDFs in the viewer, and leaves downloads alone", async ({ page, web }) => {
  await page.goto(`${web}/hello.pdf`);
  await expect(page).toHaveURL(`${VIEWER}#${web}/hello.pdf`);
  await drawn(page);
  await expect(page.locator(".text span")).toHaveText("Hello micropdf");

  const other = await page.context().newPage();
  const download = other.waitForEvent("download");
  await other.goto(`${web}/download/hello.pdf`).catch(() => {});
  expect((await download).suggestedFilename()).toBe("hello.pdf");
});

test("hands the PDF to micropdf, with the changes made here", async ({ page, web, worker, bridge }) => {
  await page.goto(`${web}/form.pdf`);
  await drawn(page);
  await page.getByRole("button", { name: "Open in micropdf" }).click();
  await expect(page.getByRole("status")).toHaveText("Opened in micropdf");
  expect(bridge[0]).toEqual({ type: "begin", name: "form.pdf" });
  expect(Buffer.from(received(bridge)).equals(readFileSync(join(FIXTURES, "form.pdf")))).toBe(true);

  // The toolbar button sends the viewer the same request.
  bridge.length = 0;
  const name = page.getByLabel("name", { exact: true });
  await name.fill("Ada");
  await name.press("Tab");
  await worker.evaluate(async () => {
    const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
    await chrome.tabs.sendMessage(tab.id!, { type: "open-in-app" });
  });
  await expect.poll(() => bridge.at(-1)?.type).toBe("end");
  const doc = mupdf.Document.openDocument(received(bridge), "application/pdf").asPDF()!;
  expect(doc.loadPage(0).getWidgets().find((w) => w.getName() === "name")?.getValue()).toBe("Ada");
});

test("follows the options: the reader's name on comments, and where PDFs open", async ({ context, page, web }) => {
  const options = await context.newPage();
  await options.goto(`chrome-extension://${ID}/options.html`);
  await options.getByLabel("Your name on comments").fill("Grace");
  await options.getByLabel("Your name on comments").press("Tab");
  await options.getByLabel("Leave it to the browser").check();

  // The rule goes once the setting is stored.
  await expect.poll(async () => {
    await page.goto(`${web}/hello.pdf`).catch(() => {});
    return page.url();
  }).toBe(`${web}/hello.pdf`);

  await options.getByLabel("Show it in micropdf's viewer").check();
  await expect.poll(async () => {
    await page.goto(`${web}/hello.pdf?again`).catch(() => {});
    return page.url().startsWith(VIEWER);
  }).toBe(true);
  await drawn(page);
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await page.getByRole("button", { name: "Note", exact: true }).click();
  const box = (await page.getByRole("group", { name: "Page 1", exact: true }).boundingBox())!;
  await page.mouse.click(box.x + box.width / 2, box.y + box.height * 0.8);
  await page.getByLabel("Comment text").fill("Signed");
  await page.getByRole("button", { name: "OK" }).click();
  await page.getByRole("button", { name: "Select", exact: true }).click();
  await page.mouse.click(box.x + box.width / 2 + 4, box.y + box.height * 0.8 + 4);
  await expect(page.getByRole("dialog", { name: "Comment" })).toContainText("Grace");
});

test("opens a PDF straight in micropdf when the reader asks for that", async ({ context, page, web, bridge }) => {
  const options = await context.newPage();
  await options.goto(`chrome-extension://${ID}/options.html`);
  await options.getByLabel("Open it in micropdf for Windows").check();
  await options.close();

  await page.goto(web);
  await page.getByRole("link", { name: "hello" }).click();
  // Handed on, and the tab goes back where it was.
  await expect.poll(() => bridge.at(-1)?.type).toBe("end");
  await expect(page).toHaveURL(`${web}/`);
  expect(bridge[0]).toEqual({ type: "begin", name: "hello.pdf" });
});

test("opens files from this computer, and saves them in place", async ({ context, page, bridge }) => {
  test.skip(process.platform !== "win32", "the bridge, and the paths it takes, are Windows only");
  // What the reader turns on with "Allow access to file URLs".
  const extensions = await context.newPage();
  await extensions.goto("chrome://extensions");
  await extensions.evaluate(
    (id) =>
      (
        chrome as unknown as {
          developerPrivate: { updateExtensionConfiguration(c: object): Promise<void> };
        }
      ).developerPrivate.updateExtensionConfiguration({ extensionId: id, fileAccess: true }),
    ID,
  );
  await extensions.close();

  const file = `file:///${FIXTURES.replaceAll("\\", "/")}/hello.pdf`;
  await expect.poll(async () => {
    await page.goto(file).catch(() => {});
    return page.url();
  }).toBe(`${VIEWER}#${file}`);
  await drawn(page);
  const local = join(FIXTURES, "hello.pdf");

  await page.getByRole("button", { name: "Open in micropdf" }).click();
  await expect(page.getByRole("status")).toHaveText("Opened in micropdf");
  expect(bridge).toEqual([{ type: "open", path: local }]);

  bridge.length = 0;
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await page.locator(".text span").selectText();
  await page.getByRole("button", { name: "Highlight", exact: true }).click();
  await page.keyboard.press("Control+s");
  await expect(page.getByRole("status")).toHaveText(`Saved ${local}`);
  expect(bridge[0]).toEqual({ type: "begin", target: local });
  const doc = mupdf.Document.openDocument(received(bridge), "application/pdf").asPDF()!;
  expect(doc.loadPage(0).getAnnotations().map((a) => a.getType())).toEqual(["Highlight"]);
});
