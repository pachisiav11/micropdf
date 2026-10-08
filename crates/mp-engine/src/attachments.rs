//! Embedded files: the document's own, in the EmbeddedFiles name tree, and files attached to
//! pages as file attachment comments.

use mupdf::Document;
use mupdf::pdf::{EmbeddedFileOptions, PdfDocument, PdfObject};

use crate::Error;
use crate::annots::{self, name, operation};

/// An embedded file and what holds it.
pub(crate) struct File {
    pub name: String,
    spec: PdfObject,
    place: Place,
}

enum Place {
    /// The EmbeddedFiles name tree, under this key.
    Tree(String),
    /// The file attachment comment `id` on `page`.
    Comment { page: usize, id: i32 },
}

impl File {
    /// The page of a file attached to a page; None for the document's own files.
    pub fn page(&self) -> Option<usize> {
        match self.place {
            Place::Comment { page, .. } => Some(page),
            Place::Tree(_) => None,
        }
    }

    /// Uncompressed size, when the document records it.
    pub fn size(&self) -> Option<usize> {
        let ef = self.spec.get_dict("EF").ok()??;
        let stream = ef.get_dict("F").ok()??;
        let size = stream.get_dict("Params").ok()??.get_dict("Size").ok()??;
        size.as_int().ok().map(|s| s.max(0) as usize)
    }

    fn data(&self) -> Result<Vec<u8>, Error> {
        let stream = self
            .spec
            .get_dict("EF")?
            .and_then(|ef| ef.get_dict("UF").ok().flatten().or(ef.get_dict("F").ok()?))
            .ok_or(Error::NotFound)?;
        Ok(stream.read_stream()?)
    }
}

/// The document's files in name tree order, then files attached to pages, page by page.
/// Empty for non-PDF documents.
pub(crate) fn list(doc: &Document) -> Result<Vec<File>, Error> {
    let Ok(pdf) = PdfDocument::try_from(doc.clone()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    // MuPDF takes the tree's name and finds it under the catalog's /Names itself.
    let tree = pdf.load_name_tree(PdfObject::new_name("EmbeddedFiles")?)?;
    for i in 0..tree.dict_len()? as i32 {
        let (Some(key), Some(spec)) = (tree.get_dict_key(i)?, tree.get_dict_val(i)?) else {
            continue;
        };
        let key = String::from_utf8_lossy(&key.as_name().unwrap_or_default()).into_owned();
        out.push(File {
            name: file_name(&spec).unwrap_or_else(|| key.clone()),
            spec,
            place: Place::Tree(key),
        });
    }
    for page in 0..pdf.page_count()?.max(0) {
        let Some(annots) = pdf.find_page(page)?.get_dict("Annots")? else {
            continue;
        };
        for i in 0..annots.len()? as i32 {
            let Some(annot) = annots.get_array(i)? else {
                continue;
            };
            if name(&annot, "Subtype")?.as_deref() != Some("FileAttachment") {
                continue;
            }
            // A file specification without /EF names a file outside the PDF.
            let Some(spec) = annot.get_dict("FS")? else {
                continue;
            };
            if spec.get_dict("EF")?.is_none() {
                continue;
            }
            out.push(File {
                name: file_name(&spec).unwrap_or_else(|| "Attachment".into()),
                spec,
                place: Place::Comment {
                    page: page as usize,
                    id: annot.as_indirect()?,
                },
            });
        }
    }
    Ok(out)
}

fn file_name(spec: &PdfObject) -> Option<String> {
    ["UF", "F"]
        .iter()
        .find_map(|k| spec.get_dict(*k).ok().flatten()?.as_string().ok())
        .filter(|n| !n.is_empty())
}

/// The contents of file `index` of [`list`].
pub(crate) fn data(doc: &Document, index: usize) -> Result<Vec<u8>, Error> {
    list(doc)?.get(index).ok_or(Error::NotFound)?.data()
}

/// Embeds `data` as a document file called `name`, under a name tree key no other file has.
pub(crate) fn add(doc: &Document, name: &str, data: &[u8]) -> Result<(), Error> {
    let mut pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
    operation(doc, "Attach file", || {
        let tree = pdf.load_name_tree(PdfObject::new_name("EmbeddedFiles")?)?;
        let mut key = name.to_owned();
        let mut n = 2;
        while tree.get_dict(key.as_str())?.is_some() {
            key = format!("{name} ({n})");
            n += 1;
        }
        pdf.add_embedded_file(&key, data, EmbeddedFileOptions::new(name))?;
        Ok(())
    })
}

/// Removes file `index` of [`list`]: from the name tree, or with the comment that holds it.
/// Returns the page that changed, if any.
pub(crate) fn delete(doc: &Document, index: usize) -> Result<Option<usize>, Error> {
    let files = list(doc)?;
    let file = files.get(index).ok_or(Error::NotFound)?;
    match &file.place {
        Place::Comment { page, id } => {
            annots::delete(doc, *page, *id)?;
            Ok(Some(*page))
        }
        Place::Tree(key) => {
            let pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
            let spec = file.spec.as_indirect()?;
            operation(doc, "Delete attachment", || {
                let root = match pdf.catalog()?.get_dict("Names")? {
                    Some(names) => names.get_dict("EmbeddedFiles")?,
                    None => None,
                };
                match root {
                    Some(root) if remove_from_tree(&root, key, spec)? => Ok(None),
                    _ => Err(Error::NotFound),
                }
            })
        }
    }
}

/// Removes the entry for `key`, or for the file specification object `spec`, from the name
/// tree under `node`. True when it was there.
fn remove_from_tree(node: &PdfObject, key: &str, spec: i32) -> Result<bool, Error> {
    if let Some(mut names) = node.get_dict("Names")? {
        let mut i = 0;
        while i + 1 < names.len()? as i32 {
            let k = names.get_array(i)?.and_then(|k| k.as_string().ok());
            let v = names
                .get_array(i + 1)?
                .map(|v| v.as_indirect())
                .transpose()?;
            if k.as_deref() == Some(key) || (spec > 0 && v == Some(spec)) {
                names.array_delete(i + 1)?;
                names.array_delete(i)?;
                return Ok(true);
            }
            i += 2;
        }
    }
    if let Some(kids) = node.get_dict("Kids")? {
        for i in 0..kids.len()? as i32 {
            if let Some(kid) = kids.get_array(i)?
                && remove_from_tree(&kid, key, spec)?
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
