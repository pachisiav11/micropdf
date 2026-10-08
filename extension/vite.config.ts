import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";

// Builds the extension into dist/, which loads unpacked: the viewer and options pages, the service worker
// (kept at the root as background.js, where the manifest points) and the mupdf.js worker, with
// public/ (the manifest and icons) copied as is.
export default defineConfig({
  plugins: [svelte()],
  base: "./",
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "chrome128",
    // mupdf's WebAssembly is about 10 MB; it loads once, in the worker.
    chunkSizeWarningLimit: 12000,
    rolldownOptions: {
      input: {
        viewer: "viewer.html",
        options: "options.html",
        background: "src/background.ts",
      },
      output: {
        entryFileNames: (chunk) =>
          chunk.name === "background" ? "background.js" : "assets/[name]-[hash].js",
      },
    },
  },
  worker: { format: "es" },
  // Pre-bundling would move mupdf away from the .wasm file it loads by a relative URL.
  optimizeDeps: { exclude: ["mupdf"] },
  // The dev server may read the test PDFs, so the viewer can open them from its own origin.
  server: { fs: { allow: [".", "../fixtures"] } },
  test: { include: ["src/**/*.test.ts"] },
});
