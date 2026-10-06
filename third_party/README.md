# Vendored crates

## mupdf-rs

`mupdf` 0.8.0 from crates.io (<https://github.com/messense/mupdf-rs>, AGPL-3.0), patched in via
`[patch.crates-io]` in the workspace `Cargo.toml`. Only `src/`, `Cargo.toml`, `LICENSE` and
`README.md` are kept; the upstream tests and examples are dropped.

Changes:

- `Context::shrink_store(percent)` — wraps `fz_shrink_store`. `mupdf-sys` creates the base
  context with a fixed 256 MB resource store; micropdf shrinks it after renders to stay within
  its memory budget.

Upstream these changes before bumping the version, then drop the vendored copy.
