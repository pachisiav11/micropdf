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

Upstream these changes before bumping the version, then drop the vendored copy.
