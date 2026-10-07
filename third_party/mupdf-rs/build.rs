// Compiles the journal (undo/redo) wrappers; see shim/journal.c.
fn main() {
    println!("cargo:rerun-if-changed=shim/journal.c");
    cc::Build::new()
        .file("shim/journal.c")
        .compile("mupdf_rs_journal");
}
