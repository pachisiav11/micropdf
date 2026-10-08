import { resolve } from "node:path";
import { type Page, expect, test } from "@playwright/test";

const FIXTURES = resolve(import.meta.dirname, "../../fixtures").replaceAll("\\", "/");

/** Opens a test PDF in the viewer, as the extension's redirect would. */
async function open(page: Page, name: string, fragment = ""): Promise<void> {
  await page.goto(`/viewer.html#http://localhost:5174/@fs/${FIXTURES}/${name}${fragment}`);
}

/** Waits until page `index` has drawn, and returns its base canvas's pixel width. */
async function drawn(page: Page, index = 0): Promise<void> {
  const canvas = page.getByRole("group", { name: `Page ${index + 1}`, exact: true }).locator("canvas.base");
  await expect.poll(() => canvas.evaluate((c: HTMLCanvasElement) => c.width)).toBeGreaterThan(200);
}

const pageNumber = (page: Page) => page.getByLabel("Page number");

test("draws the page with a selectable text layer", async ({ page }) => {
  await open(page, "hello.pdf");
  await drawn(page);
  await expect(page).toHaveTitle("hello.pdf");
  await expect(page.locator(".text span")).toHaveText("Hello micropdf");
  await expect(pageNumber(page)).toHaveValue("1");
});

test("opens at the page the URL asks for", async ({ page }) => {
  await open(page, "outline-links.pdf", "#page=3");
  await drawn(page, 2);
  await expect(pageNumber(page)).toHaveValue("3");
});

test("follows a link inside the document, and comes back", async ({ page }) => {
  await open(page, "outline-links.pdf");
  await drawn(page);
  await page.getByRole("button", { name: "Go to page 3" }).click();
  await expect(pageNumber(page)).toHaveValue("3");
  await page.keyboard.press("Alt+ArrowLeft");
  await expect(pageNumber(page)).toHaveValue("1");
  await page.keyboard.press("Alt+ArrowRight");
  await expect(pageNumber(page)).toHaveValue("3");
});

test("opens web links in a new tab, and nothing else", async ({ page }) => {
  await open(page, "outline-links.pdf");
  await drawn(page);
  const link = page.locator("a.link");
  await expect(link).toHaveAttribute("href", "https://example.com/");
  await expect(link).toHaveAttribute("target", "_blank");
  await expect(link).toHaveAttribute("rel", "noopener noreferrer");
});

test("shows thumbnails and bookmarks in the sidebar", async ({ page }) => {
  await open(page, "outline-links.pdf");
  await drawn(page);
  await page.keyboard.press("F4");
  const thumbs = page.locator(".thumb");
  await expect(thumbs).toHaveCount(3);
  await expect(thumbs.first()).toHaveAttribute("aria-current", "page");
  await thumbs.nth(1).click();
  await expect(pageNumber(page)).toHaveValue("2");

  await page.getByRole("tab", { name: "Bookmarks" }).click();
  await page.getByRole("button", { name: "Chapter 3" }).click();
  await expect(pageNumber(page)).toHaveValue("3");
});

test("finds text on every page and steps through the hits", async ({ page }) => {
  await open(page, "outline-links.pdf");
  await drawn(page);
  await page.keyboard.press("Control+f");
  await page.getByLabel("Find in document").fill("chapter");
  const status = page.locator(".find .status");
  await expect(status).toHaveText(/^1 of [4-9]$/);
  await expect(page.locator(".hit.current")).toHaveCount(1);
  await page.getByLabel("Find in document").press("Enter");
  await expect(status).toHaveText(/^2 of /);
  await page.getByLabel("Find in document").press("Escape");
  await expect(page.locator(".find")).toHaveCount(0);
  await expect(page.locator(".hit")).toHaveCount(0);
});

test("says when nothing matches", async ({ page }) => {
  await open(page, "hello.pdf");
  await drawn(page);
  await page.keyboard.press("Control+f");
  await page.getByLabel("Find in document").fill("zebra");
  await expect(page.locator(".find .status")).toHaveText("No matches");
});

test("zooms with the keyboard and fits the page", async ({ page }) => {
  await open(page, "outline-links.pdf");
  await drawn(page);
  const percent = page.locator(".percent");
  const before = Number.parseInt((await percent.textContent()) ?? "", 10);
  await page.keyboard.press("Control+=");
  await expect.poll(async () => Number.parseInt((await percent.textContent()) ?? "", 10)).toBeGreaterThan(before);
  await page.keyboard.press("Control+0");
  const frame = page.getByRole("group", { name: "Page 1", exact: true });
  const box = (await frame.boundingBox())!;
  const view = (await page.locator("main").boundingBox())!;
  expect(box.height).toBeLessThanOrEqual(view.height);
});

test("draws a sharp tile over the view when zoomed far in", async ({ page }) => {
  await open(page, "outline-links.pdf");
  await drawn(page);
  for (let i = 0; i < 12; i++) await page.keyboard.press("Control+=");
  await expect(page.locator(".percent")).toHaveText("1200%");
  const tile = page.getByRole("group", { name: "Page 1", exact: true }).locator("canvas.detail");
  await expect(tile).toBeVisible();
  await expect.poll(() => tile.evaluate((c: HTMLCanvasElement) => c.width)).toBeGreaterThan(500);
});

test("lays pages out two by two, and as a book", async ({ page }) => {
  await open(page, "outline-links.pdf");
  await drawn(page);
  const top = (n: number) =>
    page.getByRole("group", { name: `Page ${n}`, exact: true }).evaluate((e: HTMLElement) => e.style.top);
  await page.getByRole("button", { name: "View" }).click();
  await page.getByRole("button", { name: "Two pages" }).click();
  expect(await top(1)).toBe(await top(2));
  await page.getByRole("button", { name: "Book (cover alone)" }).click();
  expect(await top(2)).toBe(await top(3));
  expect(await top(1)).not.toBe(await top(2));
  await page.getByRole("button", { name: "Single page" }).click();
  await expect(page.locator(".page")).toHaveCount(1);
});

test("rotates the view", async ({ page }) => {
  await open(page, "hello.pdf");
  await drawn(page);
  const frame = page.getByRole("group", { name: "Page 1", exact: true });
  const before = (await frame.boundingBox())!;
  expect(before.width).toBeGreaterThan(before.height);
  await page.keyboard.press("Control+Shift+Equal");
  await expect.poll(async () => (await frame.boundingBox())!.height > (await frame.boundingBox())!.width).toBe(true);
});

test("recolours pages in a reading mode", async ({ page }) => {
  await open(page, "hello.pdf");
  await drawn(page);
  await page.getByRole("button", { name: "View" }).click();
  await page.getByRole("button", { name: "Sepia" }).click();
  const corner = () =>
    page.locator("canvas.base").evaluate((c: HTMLCanvasElement) => [...c.getContext("2d")!.getImageData(1, 1, 1, 1).data]);
  await expect.poll(corner).toEqual([0xf4, 0xec, 0xd8, 255]);
});

test("moves with Vim keys once they are on", async ({ page }) => {
  await open(page, "outline-links.pdf");
  await drawn(page);
  await page.keyboard.press("Shift+G");
  await expect(pageNumber(page)).toHaveValue("1");
  await page.getByRole("button", { name: "View" }).click();
  await page.getByLabel("Vim keys").check();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Shift+G");
  await expect(pageNumber(page)).toHaveValue("3");
  await page.keyboard.press("g");
  await page.keyboard.press("g");
  await expect(pageNumber(page)).toHaveValue("1");
  await page.keyboard.press("2");
  await page.keyboard.press("Shift+G");
  await expect(pageNumber(page)).toHaveValue("2");
});

test("asks for the password of an encrypted PDF", async ({ page }) => {
  await open(page, "encrypted.pdf");
  const field = page.getByLabel("This PDF has a password.");
  await field.fill("wrong");
  await field.press("Enter");
  await expect(page.getByText("That password is not right.")).toBeVisible();
  await page.locator("#password").fill("user");
  await page.locator("#password").press("Enter");
  await drawn(page);
  await expect(page.locator(".text span")).toHaveText("Hello micropdf");
});

test("explains a file that is not a PDF", async ({ page }) => {
  await open(page, "not-a-pdf.pdf");
  await expect(page.getByText(/^Could not open the PDF: /)).toBeVisible();
});

test("presents one page at a time", async ({ page }) => {
  await open(page, "outline-links.pdf");
  await drawn(page);
  await page.keyboard.press("Control+l");
  await expect(page.locator(".toolbar")).toHaveCount(0);
  await expect(page.locator(".page")).toHaveCount(1);
  await page.keyboard.press("ArrowRight");
  await expect(page.getByRole("group", { name: "Page 2", exact: true })).toBeVisible();
  await page.keyboard.press(" ");
  await expect(page.getByRole("group", { name: "Page 3", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator(".toolbar")).toHaveCount(1);
  await expect(page.locator(".page")).toHaveCount(3);
  await expect(pageNumber(page)).toHaveValue("3");
});
