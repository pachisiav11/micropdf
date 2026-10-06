//! Writes fixtures/encrypted.pdf: hello.pdf with AES-256, user password "user", owner "owner".

use std::path::PathBuf;

use mupdf::pdf::document::Encryption;
use mupdf::pdf::{PdfDocument, PdfWriteOptions};

fn main() -> Result<(), mupdf::Error> {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let source = fixtures.join("hello.pdf");
    let target = fixtures.join("encrypted.pdf");

    let doc = PdfDocument::open(source.to_str().expect("UTF-8 path"))?;
    let mut options = PdfWriteOptions::default();
    options
        .set_encryption(Encryption::Aes256)
        .set_user_password("user")
        .set_owner_password("owner");
    doc.save_with_options(target.to_str().expect("UTF-8 path"), options)?;
    println!("wrote {}", target.display());
    Ok(())
}
