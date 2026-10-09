//! Digital signatures: signing a field with a certificate from the user's Windows store (smart
//! cards and USB tokens included) or a .pfx file, certifying, and checking the signatures a
//! document carries.

use mupdf::Document;
use mupdf::pdf::{
    PdfDocument, PdfObject, PdfPage, PdfSigner, SIGNATURE_APPEARANCE, SignatureError, WidgetType,
};

pub use mupdf::pdf::{Certificate, certificates};

use crate::pages::pdf;
use crate::{DocId, Error, Rect};

/// Whose certificate signs.
#[derive(Debug, Clone)]
pub enum SignWith {
    /// A certificate in the user's personal store, by SHA-1 thumbprint.
    Store([u8; 20]),
    /// The bytes of a .pfx or .p12 file and its password.
    Pfx { data: Vec<u8>, password: String },
}

/// Which field the signature goes in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignField {
    /// An empty signature field, by widget id.
    Existing { page: usize, id: i32 },
    /// A new field at `rect` (page space).
    New { page: usize, rect: Rect },
}

#[derive(Debug, Clone)]
pub struct Signing {
    pub with: SignWith,
    pub field: SignField,
    pub reason: String,
    pub location: String,
    /// The URL of an RFC 3161 timestamp authority, for a trusted signing time.
    pub tsa: Option<String>,
    /// Certify the document: later changes other than filling in forms and signing break the
    /// signature.
    pub certify: bool,
}

/// How far a signature can be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Windows trusts the certificate's chain.
    Trusted,
    /// The certificate signed itself, or its chain ends in a root Windows does not trust.
    Unknown,
    /// The chain is broken, expired or revoked, or the signature holds no certificate.
    Untrusted,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    pub page: usize,
    /// The widget's object number.
    pub id: i32,
    pub name: String,
    pub rect: Rect,
    /// False for an empty field waiting for a signature.
    pub signed: bool,
    /// The signed bytes are unchanged.
    pub intact: bool,
    pub trust: Trust,
    /// The file was added to after signing; whether that was allowed is not judged.
    pub changed_after: bool,
    pub signer: String,
    /// The signing time the signer's computer gave, as a PDF date string.
    pub date: String,
    pub reason: String,
    pub location: String,
    pub certifies: bool,
}

fn pdf_page(doc: &Document, page: usize) -> Result<PdfPage, Error> {
    PdfPage::try_from(doc.load_page(page as i32)?).map_err(|_| Error::NotPdf)
}

fn text(dict: &PdfObject, key: &str) -> Result<String, Error> {
    Ok(match dict.get_dict(key)? {
        Some(v) => v.as_string()?,
        None => String::new(),
    })
}

/// Signs per `how`; the signature is computed when the document is next saved.
fn sign(doc: &Document, how: &Signing) -> Result<(), Error> {
    let mut pdf = pdf(doc)?;
    let tsa = how.tsa.as_deref().filter(|t| !t.trim().is_empty());
    let signer = match &how.with {
        SignWith::Store(thumbprint) => PdfSigner::from_store(thumbprint, tsa)?,
        SignWith::Pfx { data, password } => PdfSigner::from_pfx(data, password, tsa)?,
    };
    let mut widget = match how.field {
        SignField::Existing { page, id } => {
            let widget = pdf_page(doc, page)?
                .load_widget(id)?
                .ok_or(Error::NotFound)?;
            if widget.r#type()? != WidgetType::Signature {
                return Err(Error::Invalid("the field is not a signature field"));
            }
            if widget.is_signed()? {
                return Err(Error::Invalid("the field is signed already"));
            }
            widget
        }
        SignField::New { page, rect } => {
            let mut p = pdf_page(doc, page)?;
            let taken: Vec<String> = signatures(doc)?.into_iter().map(|s| s.name).collect();
            let name = (1..)
                .map(|n| format!("Signature{n}"))
                .find(|n| !taken.contains(n))
                .expect("some name is free");
            let mut widget = p.add_signature_widget(&name)?;
            widget
                .annotation_mut()
                .set_rect(mupdf::Rect::new(rect.x0, rect.y0, rect.x1, rect.y1))?;
            widget.update()?;
            widget
        }
    };
    widget.sign(&signer, SIGNATURE_APPEARANCE, &how.reason, &how.location)?;
    let field = widget.annotation().object();
    let mut v = field.get_dict("V")?.ok_or(Error::NotFound)?;
    pdf.amend_signature(|pdf| -> Result<(), Error> {
        // PAdES: CMS with the signing certificate named in a signed attribute.
        v.dict_put("SubFilter", PdfObject::new_name("ETSI.CAdES.detached")?)?;
        for (key, value) in [("Reason", &how.reason), ("Location", &how.location)] {
            if !value.is_empty() {
                v.dict_put(key, PdfObject::new_string(value)?)?;
            }
        }
        if !how.certify {
            return Ok(());
        }
        let mut reference = pdf.new_dict()?;
        reference.dict_put("Type", PdfObject::new_name("SigRef")?)?;
        reference.dict_put("TransformMethod", PdfObject::new_name("DocMDP")?)?;
        let mut params = pdf.new_dict()?;
        params.dict_put("Type", PdfObject::new_name("TransformParams")?)?;
        // 2: filling in forms and signing are allowed.
        params.dict_put("P", PdfObject::new_int(2)?)?;
        params.dict_put("V", PdfObject::new_name("1.2")?)?;
        reference.dict_put("TransformParams", params)?;
        match v.get_dict("Reference")? {
            Some(mut list) => list.array_push(reference)?,
            None => {
                let mut list = pdf.new_array()?;
                list.array_push(reference)?;
                v.dict_put("Reference", list)?;
            }
        }
        let mut perms = pdf.new_dict()?;
        perms.dict_put("DocMDP", v)?;
        pdf.catalog()?.dict_put("Perms", perms)?;
        Ok(())
    })?;
    Ok(())
}

/// Whether the form's field tree holds a signature field; far cheaper than loading each page.
fn has_signature_fields(fields: &PdfObject, depth: usize) -> Result<bool, Error> {
    if depth > 32 {
        return Ok(false);
    }
    for i in 0..fields.len()? as i32 {
        let Some(field) = fields.get_array(i)? else {
            continue;
        };
        if let Some(kind) = field.get_dict_inheritable("FT")?
            && kind.as_name()? == b"Sig"
        {
            return Ok(true);
        }
        if let Some(kids) = field.get_dict("Kids")?
            && has_signature_fields(&kids, depth + 1)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Every signature field of the document, signed or not, with what checking found.
pub(crate) fn signatures(doc: &Document) -> Result<Vec<Signature>, Error> {
    let Ok(pdf) = PdfDocument::try_from(doc.clone()) else {
        return Ok(Vec::new());
    };
    let fields = match pdf.catalog()?.get_dict("AcroForm")? {
        Some(form) => form.get_dict("Fields")?,
        None => None,
    };
    match fields {
        Some(fields) if has_signature_fields(&fields, 0)? => {}
        _ => return Ok(Vec::new()),
    }
    let certified = match pdf.catalog()?.get_dict("Perms")? {
        Some(perms) => perms
            .get_dict("DocMDP")?
            .map(|d| d.as_indirect())
            .transpose()?,
        None => None,
    };
    let mut out = Vec::new();
    for page in 0..pdf.page_count()? as usize {
        for widget in pdf_page(doc, page)?.widgets() {
            if widget.r#type()? != WidgetType::Signature {
                continue;
            }
            let field = widget.annotation().object();
            let v = field.get_dict("V")?;
            let signed = widget.is_signed()?;
            let mut s = Signature {
                page,
                id: widget.xref()?,
                name: widget.name()?.unwrap_or_default(),
                rect: widget.annotation().bounds()?.into(),
                signed,
                intact: false,
                trust: Trust::Untrusted,
                changed_after: false,
                signer: String::new(),
                date: String::new(),
                reason: String::new(),
                location: String::new(),
                certifies: false,
            };
            if let (true, Some(v)) = (signed, v) {
                let check = widget.check_signature()?;
                s.intact = check.digest == SignatureError::Okay;
                s.trust = match check.certificate {
                    SignatureError::Okay => Trust::Trusted,
                    SignatureError::SelfSigned | SignatureError::SelfSignedInChain => {
                        Trust::Unknown
                    }
                    _ => Trust::Untrusted,
                };
                s.changed_after = check.changed;
                s.signer = check.signer;
                s.date = text(&v, "M")?;
                s.reason = text(&v, "Reason")?;
                s.location = text(&v, "Location")?;
                s.certifies = v.is_indirect()? && certified == Some(v.as_indirect()?);
            }
            out.push(s);
        }
    }
    Ok(out)
}

impl crate::Engine {
    /// Signs per `how` as an undoable step. Save the document next: that computes the
    /// signature, and an incremental save keeps earlier signatures valid.
    pub fn sign(&self, doc: DocId, how: Signing) -> Result<(), Error> {
        self.edit(doc, "Sign", move |d| sign(d, &how))
    }

    pub fn signatures(&self, doc: DocId) -> Result<Vec<Signature>, Error> {
        self.read(doc, |d, _| signatures(d))
    }
}
