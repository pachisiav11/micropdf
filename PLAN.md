# micropdf — Build Plan

Drafted 2026-10-07. Working name: **micropdf**. Status: plan only, nothing built yet.

A lightweight Windows PDF reader/editor that matches Acrobat Pro's feature set, uses a fraction of
its memory, and ships with a Chromium extension that shares its engine and look.

---

## 1. Decisions

| Area | Decision | Why |
|---|---|---|
| Scope | Full Acrobat Pro parity (minus cloud services and XFA), delivered in milestones | Requested; milestones keep each release usable |
| Desktop UI | **Native Rust + Slint**, no webview | WebView2 alone costs ~100–180 MB; Slint targets ~20–50 MB |
| Slint renderer | **Software renderer** (decided in M0) | 8.6 MB idle vs 23.3 MB for FemtoVG, faster scrolling; Skia clashes with MuPDF's libjpeg (bench/README.md) |
| PDF engine | **MuPDF** (C, via `mupdf-sys`) on desktop, **mupdf.js** (WASM) in the extension | One engine everywhere; built-in redaction, annotations, forms, signatures hooks, DOCX writer, Story layout API, undo journal |
| License | **AGPL-3.0-or-later** for the whole repo | Required by MuPDF/mupdf.js; Slint's GPLv3 option is compatible |
| Platform | **Windows only** | Lets us use Windows OCR, cert store, GDI printing, Credential Manager freely |
| Look | **Recto's "workbench & sheet"** design — graphite chrome, the page is the only lit surface, blue/red pencil accents, Bahnschrift / Sitka Text / Cascadia Mono | Family resemblance with Recto |
| Extension | MV3, **TypeScript + Svelte 5 + mupdf.js**, Recto tokens | Native DOM text selection, browser find, screen readers |
| Extension roles | Replace browser PDF viewer · hand off to desktop · save page as PDF · annotate web PDFs | All four requested |
| AI | **Chat + summarize** only, Recto-style side panel; Anthropic / OpenAI / Google, bring-your-own key, exact model name required | Port of Recto's `ai.rs` |
| Extras | Tabs + split view + Ctrl+K palette · library with full-text search · Vim key layer · reading modes (dark/sepia/invert/reflow) | Selected |
| Office conversion | Nothing bundled. LibreOffice is an **optional add-on, downloadable or removable at any time** from Settings | Keeps the installer small |
| XFA | Detect and explain; show the static fallback page if the file has one | MuPDF cannot render XFA; own XFA layout is months of work |
| Relation to PDFX | **Separate project, no code reuse** | Decided |

---

## 2. Targets

Measured as **private working set summed over all processes**, on a 1080p display, with the same
files opened in Acrobat Reader for comparison (Acrobat.exe + every AcroCEF/RdrCEF child).

| Scenario | micropdf target | Acrobat (record in M0) |
|---|---|---|
| App open, no document | ≤ 30 MB | |
| One 300-page text PDF, scrolled end to end | ≤ 80 MB | |
| Five tabs of mixed PDFs | ≤ 150 MB | |
| One 1000-page scanned PDF, scrolled end to end | ≤ 120 MB | |
| Library indexer running | ≤ +40 MB while active, released after | |

| Other | Target |
|---|---|
| Cold start to first rendered page | ≤ 400 ms |
| Warm start to first rendered page | ≤ 150 ms |
| Scrolling | 60 fps at 1440p on integrated graphics |
| Installer | ≤ 20 MB |

Tile-cache size scales with screen pixels, so 4K numbers will be higher; the budget is defined
per megapixel of viewport (see §3.4).

Every milestone ends with the memory benchmark (§9) and fails if any scenario regresses > 10%.

---

## 3. Architecture

### 3.1 Process model

```
┌──────────────────────────── micropdf.exe (single process) ────────────────────────────┐
│                                                                                       │
│  Slint UI thread ──commands──►  Engine thread  (owns every pdf_document + undo journal)│
│       ▲   tiles, overlays,          │  display lists                                  │
│       │   models                    ▼                                                 │
│       └────────────────────  Render pool (N workers, cloned fz_context)               │
│                                                                                       │
│  Services:  library indexer (background-priority thread) · AI client (tokio)          │
│             Windows integrations (OCR, cert store, print, shell, COM)                 │
└───────────────▲───────────────────────────────────────────────────────────────────────┘
                │ named pipe  \\.\pipe\micropdf-<user SID>
      micropdf-bridge.exe  ◄── native messaging (stdio) ──  Chrome / Edge / Brave extension
                                                             viewer.html: Svelte + mupdf.js (Worker)
```

One process on purpose: Acrobat's multi-process CEF design is where most of its memory goes.
The trade-off (no parser sandbox) is handled in §11.

### 3.2 Repository layout

```
micropdf/
├─ Cargo.toml                 workspace
├─ crates/
│  ├─ mp-engine/              safe wrapper over mupdf-sys: open/render/stext/annots/forms/
│  │                          redact/sign/story/writers; locks + context cloning
│  ├─ mp-core/                UI-agnostic app model: sessions, page layout, tile scheduler,
│  │                          tile cache, selection, search, commands, undo (journal)
│  ├─ mp-win/                 Windows.Media.Ocr, cert store signer/verifier, GDI printing,
│  │                          shell (file assoc, jump list), single-instance pipe,
│  │                          Credential Manager, Office COM
│  ├─ mp-ai/                  port of Recto's ai.rs: providers, history, usage
│  ├─ mp-library/             tantivy index, folder watcher (notify), thumbnail cache
│  ├─ mp-convert/             LibreOffice add-on manager, Office COM bridge, PDF→DOCX/XLSX
│  └─ mp-bridge/              native messaging host exe (tiny, no MuPDF)
├─ app/                       Slint desktop app (micropdf.exe)
│  ├─ ui/*.slint
│  └─ src/
├─ extension/                 MV3 extension (Vite + Svelte 5 + TS + mupdf.js)
├─ design/tokens.json         single source of design tokens → generates tokens.slint + tokens.css
├─ installer/                 per-user installer (no admin), native-messaging + file-assoc registration
├─ bench/                     memory + startup benchmark harness, Acrobat comparison script
├─ fixtures/                  small test PDFs (licence-checked); large ones fetched by script
├─ scripts/                   fetch-fixtures, token generation, release helpers
├─ .github/                   CI workflows, issue templates, dependabot, SECURITY.md
└─ docs/
```

### 3.3 Engine wrapper (`mp-engine`)

- Start from the `mupdf` crate (0.8) and drop to `mupdf-sys` where it lacks API: custom
  `fz_locks_context`, `pdf_pkcs7_signer` / `pdf_pkcs7_verifier`, `fz_story`, redaction options,
  journal (undo) API, document writers. Fork or upstream patches as needed; pin the MuPDF version.
- Build flags: drop MuPDF's bundled CJK/Noto fonts (`TOFU_CJK*`) and install a Windows system-font
  loader instead (as SumatraPDF does) — saves ~20+ MB of binary. Keep `extract` (DOCX writer) and
  mujs (form JavaScript).
- Threading: MuPDF documents are not thread-safe. The **engine thread** owns all documents and
  runs every mutation and query as a command (actor model, oneshot replies). Render workers only
  ever touch `fz_display_list`s, each with a cloned `fz_context`. Long jobs take an `fz_cookie`
  so scrolling away cancels them.
- File access: open files through a custom `fz_stream` using
  `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`, so micropdf never locks a PDF and can
  save over the file it is reading (atomic temp-file + `ReplaceFileW`).

### 3.4 Rendering pipeline

1. Page geometry for the whole document is computed in `mp-core` from page sizes only (cheap).
2. On first view a page's **display list** is built on the engine thread and kept in an LRU
   (capped by count and bytes).
3. The viewport is cut into **512×512 device-pixel tiles**. The scheduler queues: visible tiles →
   one viewport of prefetch in scroll direction → thumbnails. Stale jobs are cancelled.
4. **Tile cache** LRU capped at ~3× viewport bytes (≈ 24 MB at 1080p).
5. Zoom/pinch: existing tiles are scaled immediately (blurry), then re-rendered at the new scale.
   A low-res page image fills gaps while tiles arrive.
6. Slint draws tiles as `Image` elements from a Rust model. Overlays (selection, search hits,
   annotation handles, active form widget) are Slint elements driven by models. Slint never
   computes PDF geometry.
7. Slint uses its **software renderer** (M0 benchmark; see bench/README.md).
8. MuPDF's resource store must be capped (it is fixed at 256 MB by `mupdf-sys`'s C wrapper); M1
   patches this, or shrinks the store explicitly, so scanned documents stay within budget.

### 3.5 Editing and saving

- Every edit is an engine command wrapped in a MuPDF journal operation → undo/redo for free,
  named ("Add highlight", "Delete pages 3–5").
- Save is **incremental** when the file is signed or only annotations changed (keeps existing
  signatures valid); **full rewrite with garbage collection** on Save As / Optimize.
- Atomic save: write temp file in the same folder → flush → `ReplaceFileW`.
- Crash recovery: periodic journal snapshot to `%LOCALAPPDATA%\micropdf\recovery`; on next launch,
  offer to restore (Recto's behaviour).

### 3.6 Design system

- `design/tokens.json` holds Recto's dark and light tokens (from `md-render/renderer/styles/themes.css`).
  `scripts/gen-tokens.mjs` (Node, no dependencies) generates `app/ui/tokens.slint` (a
  `global Theme`) and `extension/src/tokens.css`; CI fails when they are stale.
- Fonts ship with Windows: **Bahnschrift** (chrome), **Sitka Text** (reflow view, assistant answers),
  **Cascadia Mono** (metadata, code). The extension falls back to system-ui when they are absent.
- One SVG icon set (Lucide, ISC licence).
- Layout: tab strip → toolbar with tool modes (**View · Comment · Fill & Sign · Organize · Edit ·
  Protect · Convert**) → contextual tool strip → left sidebar (thumbnails, outline, comments,
  signatures, attachments, layers) → centre sheet on the graphite desk → right panel (assistant,
  annotation properties) → status bar (page n / N, zoom).
- Every command is reachable from **Ctrl+K**.

---

## 4. Acrobat Pro parity

| Acrobat tool | micropdf | Milestone | Notes |
|---|---|---|---|
| View, zoom, page modes, thumbnails, bookmarks | ✔ | M1 | single / continuous / two-page / book, rotate view |
| Find, text select/copy, links, go to page, back/forward | ✔ | M1 | |
| Print | ✔ | M1 | GDI banding at printer DPI, page ranges, scale, booklet later |
| Properties, fonts list, layers (OCG), attachments | ✔ | M1 | |
| Read Mode / Full Screen / presentation | ✔ | M1 | |
| Comment (all markup, notes, free text, ink, shapes, stamps, callouts, attach file) | ✔ | M2 | reply threads, status, filter, comment list |
| Import/export comments (FDF/XFDF), summarize comments | ✔ | M2 | |
| Fill & Sign (AcroForm fill, visual signature/initials) | ✔ | M2 | form JS calc/format/validate via mujs |
| XFA forms | ✖ notice | M2 | detect, explain, show static fallback |
| Organize Pages (reorder, insert, delete, extract, rotate, split, replace) | ✔ | M5 | |
| Combine Files | ✔ | M5 | |
| Crop, page labels, header/footer, watermark, background, Bates numbering | ✔ | M5 | |
| Optimize / Reduce size | ✔ | M5 | image downsample, font subset, object streams, GC |
| Protect (AES-256 passwords, permissions, remove security) | ✔ | M5 | |
| Redact (mark, search/pattern redact, apply, sanitize document) | ✔ | M5 | true removal via MuPDF redaction |
| Scan & OCR (make searchable) | ✔ | M6 | Windows.Media.Ocr, invisible text layer |
| Certificates: sign (PAdES), certify, timestamp, validate | ✔ | M6 | Windows cert store incl. USB tokens, .pfx |
| Compare Files | ✔ | M6 | text diff + visual overlay, synced split view |
| Measure (distance, perimeter, area) | ✔ | M6 | scale from page or user-set |
| Edit PDF (edit text/images, add text/images, links, bookmarks) | ✔ | M7 | text edit = redact glyphs + re-set text (see M7) |
| Prepare Form (create/edit fields, auto-detect) | ✔ | M7 | |
| Export PDF → Word / Excel / PowerPoint / images / text / HTML | ✔ | M8 | DOCX via MuPDF; XLSX via table detection; PPTX via add-on |
| Create PDF from Office / images / HTML / web page | ✔ | M8 (+M3 for web) | Office via installed Office or LibreOffice add-on |
| AI Assistant | ✔ | M4 | chat + summarize |
| Action Wizard / batch | ✔ | M9 | batch any tool over a file list |
| Accessibility checker, reading order, tags | later | post-1.0 | |
| PDF/A, PDF/X, preflight, print production | later | post-1.0 | MuPDF has separations/overprint preview to build on |
| Share, Send for review, Request e-signatures, Document Cloud | ✖ | — | cloud services, out of scope |
| 3D / rich media | ✖ | — | out of scope |

---

## 5. Extras beyond Acrobat

- **Tabs** (Recto behaviour): drag reorder, middle-click close, Ctrl+T/W/Tab/Shift+T, per-tab
  scroll/zoom/mode, crash restore. A second launch adds a tab to the running window.
- **Split view**: same document twice (independent scroll) or two documents; optional synced
  scroll (reused by Compare).
- **Command palette** (Ctrl+K): fuzzy search over commands, recent files, outline entries, pages.
- **Library + full-text search**: watched folders (default Documents, Downloads, Desktop);
  background indexer at background priority; tantivy index in `%LOCALAPPDATA%`; search results
  with snippets that open at the hit with highlights; OCR'd text is indexed too.
- **Vim layer** (off by default, Recto's set): `j` `k` `gg` `G` `/` `n` `N` `Ctrl+D` `Ctrl+U`,
  plus `[count]G` to go to a page and `H`/`L` for previous/next page.
- **Reading modes**: page recolour to Recto dark, sepia, or invert. v1 maps paper→sheet colour and
  ink→text colour on tiles (cheap, in workers); images are excluded using image bounding boxes
  from structured text so photos are not inverted. **Reflow**: structured text → HTML → laid out
  by MuPDF's Story API at window width in Sitka Text, rendered like normal pages.

---

## 6. Chrome extension

Targets Chromium browsers (Chrome, Edge, Brave), Manifest V3.

### 6.1 Replace the browser viewer
- Dynamic `declarativeNetRequest` rule (Chrome 128+ `responseHeaders` condition): `main_frame`/
  `sub_frame`, method GET, `Content-Type: application/pdf`, excluding
  `Content-Disposition: attachment` → redirect to `viewer.html?src=<url>`. POST-generated PDFs are
  left to the browser's own viewer (they cannot be re-fetched).
- `file://` PDFs: handled via `webNavigation` when the user enables "Allow access to file URLs";
  the options page explains how.
- Setting: *When I open a PDF* → View in browser / Open in micropdf desktop / Ask.

### 6.2 Viewer
- Svelte 5 UI with Recto tokens; mupdf.js loaded lazily in a dedicated Worker only when a PDF opens.
- Canvas tiles plus a DOM **text layer** (positioned spans) for native selection, Ctrl+F and screen
  readers.
- Thumbnails, outline, search, zoom, page modes, reading modes, Vim layer — same behaviour as desktop.

### 6.3 Annotate web PDFs
- Markup, notes, ink, shapes, free text, form fill via mupdf.js.
- Save via File System Access `showSaveFilePicker` (or download). For `file://` PDFs with the
  desktop app installed, **save in place** through the bridge.

### 6.4 Hand off to desktop
- Button in the viewer, toolbar action, and "Open link in micropdf" context-menu entry.
- Extension fetches the bytes (it has the page's cookies) and streams them in chunks over native
  messaging to `micropdf-bridge.exe`, which forwards over the named pipe; the app opens a new tab.
  If the app is not running, the bridge starts it.
- Bridge registered in HKCU for Chrome, Edge and Brave; `allowed_origins` pinned to the
  extension ID (fixed via manifest `key`).

### 6.5 Save page as PDF
- Whole page or current selection. Uses `chrome.debugger` → `Page.printToPDF`; `debugger` is an
  **optional permission** requested on first use, so the base install asks for less.
- Result goes to the desktop app (if installed) or downloads.

### 6.6 Assistant in the extension
- Proxied through the desktop app over the bridge, so API keys live in one place and never in the
  browser. Without the desktop app the panel says so.

---

## 7. AI assistant (Recto port)

- Port `md-render/src-tauri/src/ai.rs` into `crates/mp-ai` (same providers, history trimming,
  provider-counted usage, error descriptions). Proposed-edit code is not ported.
- Panel matches Recto: `Ctrl+Shift+A`, model label, new-chat button, empty state with suggestions,
  key notice with "Add a key", Ctrl+Enter to send, per-answer usage and daily total (resets at
  local midnight).
- Per-document chat, persisted and keyed by content hash (survives renames).
- Context: page-tagged text (`[p. 12] …`). Answers cite pages; citations render as links that jump
  to the page and flash the cited passage.
- Large documents: if the text fits the configured budget, send it whole; otherwise retrieve the
  most relevant pages with a local BM25 pass over page chunks and say which pages were used.
- Quick actions: summarize document / current page / selection; "Explain selection" from the
  context menu.
- Scanned pages: offer OCR first (M6). Before M6, the panel says the page has no text layer.
- Keys stored in **Windows Credential Manager** (an upgrade over Recto's settings file); used only
  by Rust code.
- No network traffic unless the user sends a message.

---

## 8. Milestones

Each milestone ends with: tests green, memory benchmark within budget, the app launched and the
golden path exercised by hand (screenshots), `todo.md` / `TODO_log.md` updated, a tagged GitHub
release.

### M0 — Foundations and spikes

**Repository (first step)**
- `git init` with `main` as default branch; create **public** `pachisiav11/micropdf` with
  `gh repo create --public --source . --push`.
- First commit: `README.md` (one-paragraph pitch, status "pre-alpha, plan only", link to
  `PLAN.md`), `PLAN.md`, `todo.md`, `TODO_log.md`, AGPL-3.0 `LICENSE` (full text), `.gitignore`
  (`target/`, `node_modules/`, `extension/dist/`, `*.pdb`, `bench/results/`, `.env`, recovery and
  scratch files), `.gitattributes` (`*.pdf binary`, LF for sources).
- Repo settings: description, topics (`pdf`, `pdf-viewer`, `pdf-editor`, `rust`, `slint`,
  `mupdf`, `windows`, `chrome-extension`), Issues on, Wiki off, squash-merge only, delete branch
  on merge.
- `.github/`: `SECURITY.md` (private reporting for parser crashes), issue templates (bug with
  sample-PDF upload note, feature), `dependabot.yml` for cargo, npm and GitHub Actions.
- Branch protection on `main` requiring the CI check, switched on once CI exists (below).
- No secrets in the repo: AI test keys only in a git-ignored `.env`. Large third-party fixture
  PDFs are fetched by `scripts/fetch-fixtures` with pinned SHA-256 hashes instead of committed or
  stored in Git LFS (LFS free quota is 1 GB).

**Build and spikes**
- Cargo workspace, `THIRD-PARTY-NOTICES.md` (cargo-about), CI on `windows-latest`.
- Build `mupdf-sys` with MSVC and our flags; render page 1 of a fixture to PNG in a test.
- Slint window with generated Recto tokens; measure idle RAM with each renderer.
- Tile viewport spike: scroll a 1000-page PDF; measure fps and RAM per renderer → choose renderer.
- Engine actor + cloned-context render pool prototype; verify no data races under stress.
- `bench/`: script that opens fixtures, scrolls, samples private working set; one-time Acrobat
  comparison recorded in `bench/README.md`.
- `fixtures/`: corpus covering text, scanned, forms, signed, encrypted, huge, CJK, broken files.

**Exit:** public repo live with CI green on `main` and branch protection on; renderer chosen;
idle ≤ 30 MB and one-doc scenario ≤ 80 MB on the spike; build is reproducible in CI.

**Outcome (2026-10-07):** all met — software renderer at 8.6 MB idle and 52.8 MB peak scrolling
a 300-page document. Acrobat comparison deferred to an unattended run. The 1000-page scan peaked
at 277 MB, which moves the store cap and viewport tiles into M1.

### M1 — Viewer (v0.1)
Open (dialog, drag-drop, CLI, file association, single instance → tabs), page modes, zoom (Ctrl+wheel,
anchored touchpad pinch), thumbnails, outline, find, text selection/copy, links, history, password
open, attachments, layers, properties, print, full screen/presentation, live reload on external
change, recent files, crash restore, tabs, split view, Ctrl+K, Vim layer, reading modes, settings,
keyboard access everywhere, AccessKit names on every control.

**Exit:** opens every fixture without crash; render golden tests pass; all §2 targets met.

### M2 — Comment, forms, visual signatures (v0.2 — Reader parity)
All annotation types with properties panel, comment list with replies/status/filter, FDF/XFDF,
comment summary; AcroForm fill with JS calc/format/validate, reset, flatten; XFA notice; drawn/typed/
image signatures and initials, saved for reuse; undo/redo; incremental save.

**Exit:** annotations and filled forms round-trip and display correctly in Chrome's viewer and
Acrobat Reader (manual check list); `qpdf --check` clean.

### M3 — Chrome extension (v0.3)
`extension/` package, generated tokens, mupdf.js viewer, DNR redirect, `file://` handling,
annotate + save, `micropdf-bridge.exe` + registration, hand-off, save page as PDF, options page.

**Exit:** Playwright suite with the unpacked extension passes (open web PDF, annotate, save,
hand-off with mocked bridge); manual check in Chrome and Edge.

### M4 — AI assistant (v0.4)
`mp-ai` port, panel, Credential Manager storage, page citations, retrieval for large docs, quick
actions, extension proxy.

**Exit:** mocked-provider tests for all three providers (port Recto's tests); live smoke test with
one real key.

### M5 — Organize, protect, redact, optimize (v0.5)
Page organizer grid (drag reorder, insert from file/blank, delete, extract, rotate, replace, split
by ranges/bookmarks/size), combine files, crop, page labels, header/footer, watermark, background,
Bates numbering, optimize presets, AES-256 protect/permissions/remove, redaction (area, text,
search/regex patterns such as phone numbers and emails, apply, sanitize metadata/hidden text/JS/
attachments), metadata editor.

**Exit:** redaction tests prove removed text is absent from the saved bytes and from text extraction.

### M6 — OCR, digital signatures, compare, measure (v0.6)
- OCR: detect pages without text, render at 300 dpi, Windows.Media.Ocr (languages from installed
  Windows language packs), write an invisible text layer with a glyphless Identity-H font and
  ToUnicode maps; deskew option.
- Signatures: `pdf_pkcs7_signer` implemented on Windows CryptoAPI/CNG — signs with any certificate
  in the user's store, which covers USB tokens whose drivers register a CSP/KSP, plus `.pfx` import;
  PAdES-B-B, optional RFC 3161 timestamp (PAdES-B-T, user-set TSA URL); certify (DocMDP).
  Verification: `pdf_pkcs7_verifier` on Windows chain building, byte-range check, changes-after-
  signing report, signature panel.
- Compare: word-level text diff + visual overlay in synced split view.
- Measure tools.

**Exit:** signed files validate in Acrobat Reader; tampered fixtures are flagged.

### M7 — Edit content and Prepare Form (v0.7)
- **Edit text**: pick a text block → inline editor → on commit, remove the original glyphs with a
  text-only redaction (images and vectors untouched) and set the new text with the Story API in the
  original font when its embedded subset has the glyphs, else the closest system font (with a
  visible notice). Paragraph re-wrap within the block's box.
- Add text box; add/replace/move/resize/delete images; links; bookmark editing.
- Prepare Form: create/edit all field types, tab order, field properties, actions; auto-detect
  fields from underscores, boxes and table cells.

**Exit:** edit fixtures re-extract to the expected text; visual diff confined to the edited box.

### M8 — Conversion (v0.8)
- PDF → DOCX (MuPDF writer), PDF → XLSX (table detection on structured text → `rust_xlsxwriter`),
  PDF → PNG/JPEG/TXT/HTML/Markdown.
- Images / HTML → PDF in-engine.
- Office → PDF: use installed Word/Excel/PowerPoint via COM (`ExportAsFixedFormat`) when present;
  otherwise an installed LibreOffice; otherwise offer the **LibreOffice add-on**.
- Add-on manager (Settings → Add-ons): download official LibreOffice MSI, verify hash, extract
  per-user (`msiexec /a`, no admin) to `%LOCALAPPDATA%\micropdf\addons`, remove at any time.
  PDF → PPTX goes through the add-on.

**Exit:** conversion fixtures produce files that open in Office/LibreOffice; add-on install and
removal leave no residue.

### M9 — Library and batch (v0.9)
Library view (grid/list, recent, folders), watched folders, tantivy index, search with snippets,
open-at-hit; batch runner for any tool over many files.

**Exit:** index 1000 fixture PDFs within the memory budget; search < 100 ms.

### M10 — Hardening and 1.0
Fuzzing (cargo-fuzz on open/render/stext/save), accessibility pass with Narrator, performance pass,
installer polish, signed auto-update, code signing, Chrome Web Store + Edge Add-ons listings,
user docs, privacy page.

---

## 9. Testing and quality

| Layer | How |
|---|---|
| Engine | unit tests per command on fixtures; render golden images with a pixel-diff threshold |
| Round-trip | edit → save → reopen; `qpdf --check`; text re-extraction asserts |
| UI | Slint testing backend for element queries; scripted golden paths |
| Extension | Vitest for logic; Playwright with `--load-extension` |
| Memory/perf | `bench/` on CI (windows-latest), fails on > 10% regression |
| Robustness | cargo-fuzz corpus; every crash becomes a fixture |
| Manual | per-milestone checklist incl. cross-checks in Acrobat Reader and Chrome's viewer |

---

## 10. Packaging, updates, licensing

- **Installer:** per-user, no admin prompt (as Recto). Registers micropdf as an "Open with" handler
  for `.pdf` (Windows will not let it silently become the default; first run explains the steps),
  the native-messaging host for Chrome/Edge/Brave, and Start-menu entries.
- **Updates:** opt-in check against GitHub Releases; update manifest signed with ed25519
  (minisign); installer verified before running.
- **Code signing:** SignPath Foundation (free for OSS) or Azure Trusted Signing — choose in M10.
- **AGPL:** About dialog links to the exact source tag; extension ships under AGPL too
  (mupdf.js). Recto's `ai.rs` is the user's own code, relicensed into this repo.

---

## 11. Security and privacy

- PDFs are untrusted input parsed in-process. Mitigations: track MuPDF releases closely, fuzz our
  wrapper, cancel/time-limit jobs, never execute Launch actions, confirm before opening external
  URIs or saving attachments, form JavaScript limited to mujs with no file or network access.
- No telemetry. Network use only for: AI (on send), update check (opt-in), add-on download
  (explicit), TSA timestamp (when signing with one), links the user confirms.
- Extension asks for minimal permissions; `debugger` is optional and requested on first use.

---

## 12. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| In-place text editing fidelity (subset fonts lack glyphs) | High | font fallback with notice; keep edits box-local; same limitation exists in Acrobat |
| Safe Rust wrapper over MuPDF threading | High | engine actor, display-list-only workers, stress tests in M0 |
| Slint gaps (IME in form fields, very long virtualised lists, custom viewport) | Medium | M0 spikes; fall back to Rust-composited viewport if needed |
| `mupdf-sys` limits (256 MB store, Windows toolset detection, bundled libjpeg clashes) | Medium | patch via `[patch.crates-io]` fork when needed; avoid crates that bundle libjpeg |
| Memory budget at 4K / many tabs | Medium | per-megapixel cache budget; drop caches of background tabs |
| Chrome Web Store review of broad host permissions | Medium | clear justification, optional permissions, Edge listing in parallel |
| Signature trust differs from Acrobat (AATL vs Windows roots) | Medium | Windows root store by default; optional trust-list import |
| AGPL obligations | Low | whole repo AGPL; source link in About |
| MuPDF CVEs | Medium | pinned version, update policy, fuzzing |

---

## 13. Out of scope

macOS/Linux, mobile, XFA rendering, cloud sharing/review/e-sign requests, Acrobat JavaScript beyond
form logic, 3D and rich media, PDF/A–PDF/X conversion and preflight (post-1.0 candidates), AI
proposed edits / form fill / agent commands (not selected).

---

## 14. Assumptions to confirm

1. If Microsoft Office is installed, it is used for Office → PDF before LibreOffice.
2. Digital signing covers the Windows certificate store (including USB tokens) and `.pfx` files.
3. The extension's assistant needs the desktop app (keys never live in the browser).
4. Chromium browsers only; Firefox is not planned.
5. "micropdf" is a working name.

---

## 15. Changes during the build

- **Library (M9):** search runs over a text store on disk (`library.tsv` and `library.txt`) read
  one document at a time, not tantivy, which was not in the offline crate cache. It met the exit
  test (1000 PDFs, search well under 100 ms). Search lives in the palette (Ctrl+Shift+F) rather
  than a grid view.
- **LibreOffice add-on (M8):** offered under Convert rather than Settings → Add-ons.
- **Installer (M10):** the exe installs itself (`--install`, `--uninstall`) instead of a separate
  installer tool, so a release is one zip.
- **Updates (M10):** the manifest is signed with ECDSA P-256 and checked by Windows CNG instead
  of minisign's Ed25519, which Windows does not offer; no crypto crate is needed.
- **Fuzzing (M10):** a mutation fuzz test in the normal test suite instead of cargo-fuzz, which
  needs nightly Rust and libFuzzer; it runs in CI on every push and longer on demand.
