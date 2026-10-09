//! Passwords and permissions, Optimize, the document's properties and Sanitize.

use std::path::{Path, PathBuf};

use mupdf::Document;
use mupdf::pdf::{
    Encryption, PdfDocument, PdfObject, PdfRedactImageMethod, PdfRedactLineArtMethod,
    PdfRedactOptions, PdfRedactTextMethod, PdfWriteOptions, Permission,
};

use crate::Error;

/// What a save does to the file's password and permissions.
#[derive(Debug, Clone, Default)]
pub enum Security {
    #[default]
    Keep,
    Remove,
    Protect(Protection),
}

/// AES-256 encryption. With no `open` password anyone can open the file, and `owner` guards
/// the permissions; with no `owner` password the open password grants every permission.
#[derive(Debug, Clone, Default)]
pub struct Protection {
    pub open: String,
    pub owner: String,
    pub print: bool,
    pub copy: bool,
    /// Change pages and content.
    pub edit: bool,
    /// Add comments and fill in forms.
    pub comment: bool,
}

impl Protection {
    /// The password that opens the file again after a save.
    pub(crate) fn password(&self) -> &str {
        if self.open.is_empty() {
            &self.owner
        } else {
            &self.open
        }
    }

    fn permissions(&self) -> Permission {
        let mut p = Permission::ACCESSIBILITY;
        if self.print {
            p |= Permission::PRINT | Permission::PRINT_HQ;
        }
        if self.copy {
            p |= Permission::COPY;
        }
        if self.edit {
            p |= Permission::MODIFY | Permission::ASSEMBLE;
        }
        if self.comment {
            p |= Permission::ANNOTATE | Permission::FORM;
        }
        p
    }
}

/// What Optimize does besides rewriting the file compactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Optimize {
    /// Downsample images to this resolution and recompress JPEGs at this quality (1-100).
    pub images: Option<(i32, u8)>,
    /// Cut embedded fonts down to the glyphs in use.
    pub fonts: bool,
}

impl Optimize {
    /// Same pixels: a compact rewrite with object streams and subset fonts.
    pub const LOSSLESS: Optimize = Optimize {
        images: None,
        fonts: true,
    };
    pub const BALANCED: Optimize = Optimize {
        images: Some((150, 80)),
        fonts: true,
    };
    pub const SMALLEST: Optimize = Optimize {
        images: Some((96, 60)),
        fonts: true,
    };
}

/// What Sanitize removes.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sanitize {
    /// The document information, the XMP packet and private application data.
    pub metadata: bool,
    /// JavaScript, and actions that run something when the file opens or a page shows.
    pub scripts: bool,
    pub attachments: bool,
    /// Text that is there but not drawn (text render mode 3), such as an OCR layer.
    pub hidden_text: bool,
    pub comments: bool,
}

/// Writes the document to `target`. Incremental saves append the changes to a copy of
/// `original`, which keeps existing signatures valid; full saves rewrite and compact it.
pub(crate) fn save(
    doc: &Document,
    original: &Path,
    target: &Path,
    incremental: bool,
    security: &Security,
) -> Result<(), Error> {
    let pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
    let target_str = target.to_str().ok_or(Error::Invalid("path is not UTF-8"))?;
    let mut options = PdfWriteOptions::default();
    if incremental && pdf.can_be_saved_incrementally() {
        // MuPDF appends to the file at `target`, so it must start as the original bytes.
        if target != original {
            std::fs::copy(original, target)?;
        }
        options.set_incremental(true);
    } else {
        options.set_garbage_level(1).set_compress(true);
    }
    secure(&mut options, security);
    pdf.save_with_options(target_str, options)?;
    Ok(())
}

fn secure(options: &mut PdfWriteOptions, security: &Security) {
    match security {
        Security::Keep => {}
        Security::Remove => {
            options.set_encryption(Encryption::None);
        }
        Security::Protect(p) => {
            options
                .set_encryption(Encryption::Aes256)
                .set_user_password(&p.open)
                .set_owner_password(&p.owner)
                .set_permissions(p.permissions());
        }
    }
}

/// Writes a smaller copy of the document to `target`, leaving the open one as it is. The
/// copy keeps the file's encryption; `password` opens the in-memory copy it is made from.
pub(crate) fn optimize(
    doc: &Document,
    password: Option<&str>,
    target: &Path,
    how: Optimize,
) -> Result<(), Error> {
    let pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
    let mut full = PdfWriteOptions::default();
    full.set_garbage_level(1);
    let mut bytes = Vec::new();
    pdf.write_to_with_options(&mut bytes, full)?;
    let mut copy = PdfDocument::from_bytes(&bytes)?;
    drop(bytes);
    if copy.needs_password()? && !copy.authenticate(password.unwrap_or_default())? {
        return Err(Error::Invalid("the file's password no longer opens it"));
    }
    if let Some((dpi, quality)) = how.images {
        copy.rewrite_images(dpi, quality)?;
    }
    if how.fonts {
        copy.subset_fonts()?;
    }
    let mut options = PdfWriteOptions::default();
    options
        .set_garbage_level(3)
        .set_compress(true)
        .set_compress_fonts(true)
        .set_compress_images(true)
        .set_clean(true)
        .set_object_streams(true);
    let target = target.to_str().ok_or(Error::Invalid("path is not UTF-8"))?;
    copy.save_with_options(target, options)?;
    Ok(())
}

/// The document information fields Properties edits.
pub const INFO_FIELDS: [&str; 4] = ["Title", "Author", "Subject", "Keywords"];

/// Sets document information fields; an empty value removes the field.
pub(crate) fn set_info(doc: &Document, fields: &[(String, String)]) -> Result<(), Error> {
    let mut pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
    let mut trailer = pdf.trailer()?;
    let mut info = match trailer.get_dict("Info")? {
        Some(info) => info,
        None => {
            let info = pdf.new_dict()?;
            let info = pdf.add_object(&info)?;
            trailer.dict_put("Info", info.try_clone()?)?;
            info
        }
    };
    for (key, value) in fields {
        if !INFO_FIELDS.contains(&key.as_str()) {
            return Err(Error::Invalid("not a document information field"));
        }
        if value.trim().is_empty() {
            info.dict_delete(key.as_str())?;
        } else {
            info.dict_put(key.as_str(), PdfObject::new_string(value.trim())?)?;
        }
    }
    Ok(())
}

/// Removes what `what` asks for. Only a full save drops the removed objects from the file.
pub(crate) fn sanitize(doc: &Document, what: Sanitize) -> Result<(), Error> {
    let pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
    let mut catalog = pdf.catalog()?;
    if what.metadata {
        pdf.trailer()?.dict_delete("Info")?;
        catalog.dict_delete("Metadata")?;
        catalog.dict_delete("PieceInfo")?;
    }
    if what.scripts {
        if let Some(mut names) = catalog.get_dict("Names")? {
            names.dict_delete("JavaScript")?;
        }
        if let Some(action) = catalog.get_dict("OpenAction")?
            && !action.is_array()?
        {
            catalog.dict_delete("OpenAction")?;
        }
        catalog.dict_delete("AA")?;
        if let Some(form) = catalog.get_dict("AcroForm")?
            && let Some(fields) = form.get_dict("Fields")?
        {
            strip_field_actions(&fields, 0)?;
        }
    }
    if what.attachments
        && let Some(mut names) = catalog.get_dict("Names")?
    {
        names.dict_delete("EmbeddedFiles")?;
    }
    for i in 0..pdf.page_count()? {
        let mut page = pdf.load_pdf_page(i)?;
        let mut object = page.object();
        if what.scripts {
            object.dict_delete("AA")?;
        }
        if what.metadata {
            object.dict_delete("Metadata")?;
            object.dict_delete("PieceInfo")?;
        }
        let doomed: Vec<_> = page
            .annotations()
            .filter(|a| {
                let kind = a.r#type().ok();
                use mupdf::pdf::PdfAnnotationType as T;
                (what.comments && !matches!(kind, Some(T::Widget | T::Link)))
                    || (what.attachments && kind == Some(T::FileAttachment))
            })
            .collect();
        for annot in doomed {
            page.delete_annotation(annot)?;
        }
        if what.scripts {
            for annot in page.annotations() {
                let mut obj = annot.object();
                obj.dict_delete("AA")?;
                if let Some(action) = obj.get_dict("A")?
                    && !matches!(
                        crate::annots::name(&action, "S")?.as_deref(),
                        Some("URI" | "GoTo" | "Named")
                    )
                {
                    obj.dict_delete("A")?;
                }
            }
        }
        if what.hidden_text {
            let bounds = page.bounds()?;
            page.add_redact_annotation(bounds)?;
            page.apply_redactions_with_options(PdfRedactOptions {
                black_boxes: false,
                image_method: PdfRedactImageMethod::None,
                line_art: PdfRedactLineArtMethod::None,
                text: PdfRedactTextMethod::RemoveInvisible,
            })?;
        }
    }
    Ok(())
}

/// Drops the actions of form fields that are not widgets on a page (a widget's own go with
/// the page's annotations).
fn strip_field_actions(fields: &PdfObject, depth: usize) -> Result<(), Error> {
    if depth > 32 {
        return Ok(());
    }
    for field in fields.array_iter()? {
        let mut field = field?;
        field.dict_delete("AA")?;
        if let Some(kids) = field.get_dict("Kids")? {
            strip_field_actions(&kids, depth + 1)?;
        }
    }
    Ok(())
}

impl crate::Engine {
    /// Writes a smaller copy of the document to `target`; see [`Optimize`].
    pub fn optimize(&self, doc: crate::DocId, target: PathBuf, how: Optimize) -> Result<(), Error> {
        self.read(doc, move |d, password| optimize(d, password, &target, how))
    }

    /// Sets document information fields (see [`INFO_FIELDS`]); an empty value removes one.
    pub fn set_info(&self, doc: crate::DocId, fields: Vec<(String, String)>) -> Result<(), Error> {
        self.edit(doc, "Change document properties", move |d| {
            set_info(d, &fields)
        })
    }

    pub fn sanitize(&self, doc: crate::DocId, what: Sanitize) -> Result<(), Error> {
        self.remove(doc, "Sanitize document", move |d| sanitize(d, what))
    }
}
