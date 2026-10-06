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

## License

[AGPL-3.0-or-later](LICENSE). micropdf is built on MuPDF, which is AGPL-licensed.
