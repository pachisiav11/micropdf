# Vendored crates

## mupdf-rs

`mupdf` 0.8.0 from crates.io (<https://github.com/messense/mupdf-rs>, AGPL-3.0), patched in via
`[patch.crates-io]` in the workspace `Cargo.toml`. Only `src/`, `Cargo.toml`, `LICENSE` and
`README.md` are kept; the upstream tests and examples are dropped.

Changes:

- `Context::shrink_store(percent)` — wraps `fz_shrink_store`. `mupdf-sys` creates the base
  context with a fixed 256 MB resource store; micropdf shrinks it after renders to stay within
  its memory budget.
- `PdfDocument::layer_ui` / `toggle_layer_ui` and `LayerUi` — wrap `pdf_count_layer_config_ui`,
  `pdf_layer_config_ui_info` and `pdf_toggle_layer_config_ui` for the layers panel.
- `Font` is `Clone` (`fz_keep_font`), `Send` and `Sync`, so micropdf's system-font loader can
  hand one loaded font to every document that asks for it.
- `PdfDocument::enable_journal`, `undo`, `redo`, `undo_redo_state` and `undo_redo_step` — the
  undo journal. mupdf-sys has no `fz_try` wrappers for these, so `shim/journal.c` (compiled by
  `build.rs` with `cc`) provides them, declaring the few MuPDF 1.27 functions it calls.
- `PdfWidget::toggle` and `choice_options` — wrap `pdf_toggle_widget` and
  `pdf_choice_widget_options` through the same shim.
- `PdfAnnotation::set_appearance` — wraps `pdf_set_annot_appearance_from_display_list` through
  the same shim; micropdf draws signatures into stamps with it.
- `PdfAnnotation::set_hidden_for_editing` — wraps `pdf_set_annot_hidden_for_editing`; micropdf
  leaves comment replies out of page drawing with it.
- `PdfDocument::new_embedded_file` — the first half of `add_embedded_file`: embeds a file
  without naming it in the EmbeddedFiles tree, for file attachment comments. It also ends the
  undo operation MuPDF 1.27's `pdf_add_embedded_file` leaves open on success, which fixes
  `add_embedded_file` too.

Upstream these changes before bumping the version, then drop the vendored copy.

## mupdf-sys

`mupdf-sys` 0.8.0 from crates.io (same repository, AGPL-3.0), patched in the same way. The
package's 64 MB `mupdf` directory (the MuPDF 1.27.2 sources) is not kept: `build.rs` takes it
from the package in Cargo's download cache (`~/.cargo/registry/cache`), or downloads the package
from crates.io with `curl`, checks it against the published SHA-256 and unpacks the directory
into `OUT_DIR`. Everything else is the published crate.

Changes:

- `build.rs` — the source unpacking above, and one more source patch: MuPDF 1.27.2 leaves out the
  `NULL` that ends `pdf_dict_getl`'s key list in the JavaScript `getField` and `resetForm`
  (`source/pdf/pdf-js.c`), so `getField` returns `null` and form calculate scripts fail. MuPDF
  fixed this after 1.27.2; drop the patch when the bundled MuPDF has the fix.
- `msbuild.rs` — leaves Tesseract, Leptonica and zxing-cpp out of `libmupdf` (and turns OCR
  output and barcodes off) unless the `tesseract` or `zxingcpp` feature is on; the stock
  solution always builds them in. MuPDF always builds in its Release configuration, without
  whole-program optimization unless Rust optimizes (opt-level 2, 3, s or z): the Debug
  configuration links the debug C runtime, which clashes with the release runtime Rust links
  (LNK4098), and whole-program optimization makes every test binary's link take minutes.
- `Cargo.toml` — `flate2`, `sha2` and `tar` build dependencies; no `include` list.
