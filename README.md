# micropdf

A lightweight PDF reader and editor for Windows that aims for Acrobat Pro's feature set at a
fraction of its memory. Native Rust UI (Slint) over the MuPDF engine, in a single process, plus a
Chromium extension that shares the same engine (mupdf.js) and design.

**Status: pre-alpha.** Nothing usable yet — see [PLAN.md](PLAN.md) for the build plan and
[todo.md](todo.md) for what is in progress.

## Goals

- Idle under 30 MB, one large document under 80 MB, five tabs under 150 MB.
- Acrobat Pro parity: view, comment, forms, signatures (visual and certificate), organize, redact,
  protect, OCR, compare, edit text and images, prepare forms, convert.
- Extras: tabs and split view, command palette, library with full-text search, Vim keys, reading
  modes, and an optional bring-your-own-key AI assistant.
- No telemetry, no accounts, no network use unless you ask for it.

## Building

Requires Windows, the stable Rust toolchain (MSVC), Visual Studio Build Tools with the C++
workload, and libclang for `bindgen`. Point `LIBCLANG_PATH` at the folder containing
`libclang.dll`, normally an LLVM install (`C:\Program Files\LLVM\bin`).

The `libclang` Python wheel (`pip install --user libclang`, then `…\site-packages\clang\native`)
also works, but it ships without clang's own headers, so `max_align_t` goes missing from the
bindings. Work around it with a header containing `typedef double max_align_t;` and
`BINDGEN_EXTRA_CLANG_ARGS="-include <path-to-that-header>"`.

```sh
cargo test --workspace
```

The first build compiles MuPDF from source and takes several minutes. `mupdf-sys` only detects
Visual Studio 2019/2022 toolsets; with newer Build Tools set the toolset yourself, e.g.
`MUPDF_MSVC_PLATFORM_TOOLSET=v145` for Visual Studio 2026.

Design tokens live in `design/tokens.json`; after editing, run `node scripts/gen-tokens.mjs` to
regenerate `app/ui/tokens.slint` and `extension/src/tokens.css` (CI fails if they are stale).

## Browser extension

The extension in `extension/` opens PDFs from the web in its own viewer instead of the browser's,
where you can comment, fill in forms and save a copy. It needs Node.js 22 or later. The browser
build of MuPDF runs no JavaScript, so of a form's scripts only simple calculations (sum, product,
average, minimum, maximum) take effect there; the desktop app runs them all.

```sh
cd extension
npm install
npm run build   # or `npm run dev` to rebuild on every change
npm test        # unit tests; `npm run check` type-checks
npm run e2e     # builds, then browser tests with the extension loaded, headless in the
                # installed Chrome (and Edge on Windows); no browser download
```

Load `extension/dist` as an unpacked extension (`chrome://extensions`, Developer mode, Load
unpacked) in Chrome, Edge or Brave. The manifest carries a fixed key, so the extension ID is always
`phhaejfhblmccnkhnhjflbhckkanlnki`, the ID that the native host accepts.

To hand PDFs to the desktop app, register the native host once with
`micropdf-bridge.exe --register` (the app's "Add micropdf to Windows PDF apps" does this too).
Then "Open in micropdf" in the viewer, the toolbar button and the link menu open PDFs in the app;
on other pages the toolbar button prints the page to a PDF first (it asks for the debugger
permission once). The options page sets your name on comments and whether PDFs open in the viewer,
the app or the browser's own viewer. For PDFs on this computer (file:// links), turn on "Allow
access to file URLs" for the extension; the viewer then saves them in place through the bridge.

`npm run serve` starts Vite's dev server, where the viewer opens the test PDFs from `fixtures/`
without the extension, e.g. `http://localhost:5173/viewer.html#http://localhost:5173/@fs/<repo>/fixtures/hello.pdf`.

## License

[AGPL-3.0-or-later](LICENSE). micropdf is built on MuPDF, which is AGPL-licensed.
