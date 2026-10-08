import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { type Page, expect, test } from "@playwright/test";
import * as mupdf from "mupdf";
import { drawn, open } from "./util";

type Saved = { saved?: number[] };

// Stands in for the browser's save dialog, keeping what the viewer writes.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const w = window as unknown as Saved & Record<string, unknown>;
    w.showSaveFilePicker = async (options: { suggestedName: string }) => ({
      name: options.suggestedName,
      createWritable: async () => ({
        write: async (bytes: Uint8Array) => (w.saved = Array.from(bytes)),
        close: async () => {},
      }),
    });
  });
});

/** Saves with Ctrl+S, and opens what was written. */
async function save(page: Page): Promise<{ doc: mupdf.PDFDocument; bytes: Uint8Array }> {
  await page.evaluate(() => delete (window as Saved).saved);
  await page.keyboard.press("Control+s");
  await expect.poll(() => page.evaluate(() => !!(window as Saved).saved)).toBe(true);
  const bytes = new Uint8Array(await page.evaluate(() => (window as Saved).saved!));
  return { doc: mupdf.Document.openDocument(bytes, "application/pdf").asPDF()!, bytes };
}

const kinds = (doc: mupdf.PDFDocument) => doc.loadPage(0).getAnnotations().map((a) => a.getType());
const sheet = (page: Page) => page.getByRole("group", { name: "Page 1", exact: true });

/** A point on page 1, given as fractions of its size, in window pixels. */
async function at(page: Page, x: number, y: number): Promise<[number, number]> {
  const box = (await sheet(page).boundingBox())!;
  return [box.x + x * box.width, box.y + y * box.height];
}

async function drag(page: Page, from: [number, number], to: [number, number], steps = 5): Promise<void> {
  await page.mouse.move(...(await at(page, ...from)));
  await page.mouse.down();
  await page.mouse.move(...(await at(page, ...to)), { steps });
  await page.mouse.up();
}

async function tool(page: Page, name: string): Promise<void> {
  if (!(await page.getByRole("toolbar", { name: "Comment tools" }).isVisible())) {
    await page.getByRole("button", { name: "Comment", exact: true }).click();
  }
  await page.getByRole("button", { name, exact: true }).click();
}

test("marks selected text, and saves it after the original bytes", async ({ page }) => {
  await open(page, "hello.pdf");
  await drawn(page);
  await page.getByRole("button", { name: "Comment", exact: true }).click();
  await page.locator(".text span").selectText();
  await page.getByRole("button", { name: "Highlight", exact: true }).click();
  await expect(page.getByRole("button", { name: "Save •" })).toBeVisible();

  // With the tool on, selecting text marks it.
  await tool(page, "Strike out");
  const line = (await page.locator(".text span").boundingBox())!;
  await page.mouse.move(line.x + 2, line.y + line.height / 2);
  await page.mouse.down();
  await page.mouse.move(line.x + line.width - 2, line.y + line.height / 2, { steps: 5 });
  await page.mouse.up();

  const { doc, bytes } = await save(page);
  expect(kinds(doc)).toEqual(["Highlight", "StrikeOut"]);
  expect(doc.loadPage(0).getAnnotations()[0].getQuadPoints()).toHaveLength(1);
  const original = readFileSync(resolve(import.meta.dirname, "../../fixtures/hello.pdf"));
  expect(Buffer.from(bytes.subarray(0, original.length)).equals(original)).toBe(true);
  await expect(page.getByRole("button", { name: "Save", exact: true })).toBeVisible();
});

test("draws shapes and pen strokes, picks, deletes and undoes", async ({ page }) => {
  await open(page, "hello.pdf");
  await drawn(page);
  await tool(page, "Rectangle");
  await drag(page, [0.1, 0.6], [0.3, 0.9]);
  await tool(page, "Pen");
  await drag(page, [0.5, 0.6], [0.7, 0.9], 10);
  await tool(page, "Arrow");
  await drag(page, [0.75, 0.6], [0.9, 0.9]);
  await page.keyboard.press("Control+z");

  await tool(page, "Select");
  const card = page.getByRole("dialog", { name: "Comment" });
  await page.mouse.click(...(await at(page, 0.85, 0.75)));
  await expect(card).toHaveCount(0);
  await page.mouse.click(...(await at(page, 0.2, 0.75)));
  await expect(card).toContainText("Square");
  await page.keyboard.press("Delete");
  await expect(card).toHaveCount(0);
  await page.keyboard.press("Control+z");
  await page.keyboard.press("Control+y");
  await page.keyboard.press("Control+z");
  const first = await save(page);
  expect(kinds(first.doc)).toEqual(["Square", "Ink"]);
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();

  // A second save appends to the first.
  await tool(page, "Ellipse");
  await drag(page, [0.75, 0.6], [0.9, 0.9]);
  const second = await save(page);
  expect(kinds(second.doc)).toEqual(["Square", "Ink", "Circle"]);
  expect(Buffer.from(second.bytes.subarray(0, first.bytes.length)).equals(Buffer.from(first.bytes))).toBe(true);
});

test("adds a note, edits its text and deletes it", async ({ page }) => {
  await open(page, "hello.pdf");
  await drawn(page);
  await tool(page, "Note");
  await page.mouse.click(...(await at(page, 0.6, 0.7)));
  await page.getByLabel("Comment text").fill("Check this");
  await page.getByRole("button", { name: "OK" }).click();

  await tool(page, "Select");
  const [x, y] = await at(page, 0.6, 0.7);
  await page.mouse.click(x + 5, y + 5);
  const card = page.getByRole("dialog", { name: "Comment" });
  await expect(card).toContainText("Check this");
  await card.getByRole("button", { name: "Edit" }).click();
  await page.getByLabel("Comment text").fill("Checked");
  await page.getByRole("button", { name: "OK" }).click();
  await expect(card).toContainText("Checked");
  const { doc } = await save(page);
  expect(doc.loadPage(0).getAnnotations()[0].getContents()).toBe("Checked");

  await card.getByRole("button", { name: "Delete" }).click();
  await expect(card).toHaveCount(0);
  expect(kinds((await save(page)).doc)).toEqual([]);
});

test("fills in a form and saves the values", async ({ page }) => {
  await open(page, "form.pdf");
  await drawn(page);
  const name = page.getByLabel("name", { exact: true });
  await name.fill("Ada");
  await name.press("Tab");
  const agree = page.getByRole("checkbox", { name: "agree" });
  await agree.click();
  await expect(agree).toHaveAttribute("aria-checked", "true");
  await page.getByLabel("colour").selectOption("Blue");
  await expect(name).toHaveValue("Ada");

  const { doc } = await save(page);
  const values = Object.fromEntries(doc.loadPage(0).getWidgets().map((w) => [w.getName(), w.getValue()]));
  expect(values).toMatchObject({ name: "Ada", colour: "Blue" });
  expect(values.agree).not.toBe("Off");
});

test("runs the form's sums", async ({ page }) => {
  await open(page, "calc.pdf");
  await drawn(page);
  const field = (name: string) => page.getByLabel(name, { exact: true });
  await field("a").fill("2");
  await field("a").press("Tab");
  await field("b").fill("3");
  await field("b").press("Tab");
  await expect(field("total")).toHaveValue("5");
  await field("a").fill("2.5");
  await field("a").press("Tab");
  await expect(field("total")).toHaveValue("5.5");
});
