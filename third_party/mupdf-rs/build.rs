// Compiles the C wrappers: shim/journal.c (undo/redo and other calls mupdf-sys leaves out) and
// shim/sign.c (digital signatures on Windows CryptoAPI).
fn main() {
    println!("cargo:rerun-if-changed=shim/journal.c");
    println!("cargo:rerun-if-changed=shim/sign.c");
    cc::Build::new()
        .file("shim/journal.c")
        .file("shim/sign.c")
        .compile("mupdf_rs_journal");
    println!("cargo:rustc-link-lib=crypt32");
    println!("cargo:rustc-link-lib=ncrypt");
    println!("cargo:rustc-link-lib=advapi32");
}
