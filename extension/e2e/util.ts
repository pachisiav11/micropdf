import { resolve } from "node:path";
import { type Page, expect } from "@playwright/test";

const FIXTURES = resolve(import.meta.dirname, "../../fixtures").replaceAll("\\", "/");

/** Opens a test PDF in the viewer, as the extension's redirect would. */
export async function open(page: Page, name: string, fragment = ""): Promise<void> {
  await page.goto(`/viewer.html#http://localhost:5174/@fs/${FIXTURES}/${name}${fragment}`);
}

/** Waits until page `index` has drawn. */
export async function drawn(page: Page, index = 0): Promise<void> {
  const canvas = page.getByRole("group", { name: `Page ${index + 1}`, exact: true }).locator("canvas.base");
  await expect.poll(() => canvas.evaluate((c: HTMLCanvasElement) => c.width)).toBeGreaterThan(200);
}
