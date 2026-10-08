import { defineConfig } from "@playwright/test";

// Browser tests. The viewer ones run against Vite's dev server, which serves the test PDFs from
// fixtures/; they drive the installed Chrome, headless, so no browser download is needed.
export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  // Each page compiles mupdf's 10 MB of WebAssembly; more workers than this only queue on the CPU.
  workers: 4,
  expect: { timeout: 15_000 },
  reporter: "list",
  use: {
    channel: "chrome",
    baseURL: "http://localhost:5174",
    viewport: { width: 1100, height: 800 },
  },
  webServer: {
    command: "npm run serve -- --port 5174 --strictPort",
    url: "http://localhost:5174/viewer.html",
    reuseExistingServer: true,
  },
});
