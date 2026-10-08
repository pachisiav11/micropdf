//! Form fields: list them, fill them in and reset them. Each change is one undoable step.

use mupdf::Document;
use mupdf::pdf::{FieldFlags, PdfDocument, PdfObject, PdfPage, PdfWidget, WidgetType};

use crate::annots::operation;
use crate::{Error, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Checkbox,
    Radio,
    Choice,
    Button,
    Signature,
    Other,
}

/// One widget of a form field. `id` is the widget's object number.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub id: i32,
    pub kind: FieldKind,
    pub name: String,
    pub value: String,
    pub rect: Rect,
    pub read_only: bool,
    pub multiline: bool,
    /// The choices of a list or combo box.
    pub options: Vec<String>,
}

/// Whether a form carries XFA, Adobe's XML form format, which micropdf does not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Xfa {
    None,
    /// XFA beside ordinary AcroForm fields, which micropdf fills.
    Static,
    /// XFA only: the pages hold a placeholder, and the real form exists only in the XFA.
    Dynamic,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldEdit {
    /// New text for a text field, or the chosen option of a list or combo box.
    Value(String),
    /// Checks or unchecks a checkbox or radio button.
    Toggle,
}

pub fn xfa(doc: &Document) -> Result<Xfa, Error> {
    let Ok(pdf) = PdfDocument::try_from(doc.clone()) else {
        return Ok(Xfa::None);
    };
    let catalog = pdf.catalog()?;
    let Some(form) = catalog.get_dict("AcroForm")? else {
        return Ok(Xfa::None);
    };
    if form.get_dict("XFA")?.is_none() {
        return Ok(Xfa::None);
    }
    let needs_rendering = match catalog.get_dict("NeedsRendering")? {
        Some(v) => v.as_bool()?,
        None => false,
    };
    let fields = match form.get_dict("Fields")? {
        Some(f) => f.len()?,
        None => 0,
    };
    Ok(if needs_rendering || fields == 0 {
        Xfa::Dynamic
    } else {
        Xfa::Static
    })
}

/// Prepares the document for a change to its fields: turns on form JavaScript, so calculate,
/// format and validate actions run, and removes any XFA packet, which readers such as Acrobat
/// would otherwise show instead of the new AcroForm values.
fn prepare(pdf: &mut PdfDocument) -> Result<(), Error> {
    if !pdf.is_js_supported()? {
        pdf.enable_js()?;
    }
    if let Some(mut form) = pdf.catalog()?.get_dict("AcroForm")?
        && form.get_dict("XFA")?.is_some()
    {
        form.dict_delete("XFA")?;
    }
    Ok(())
}

fn pdf_page(doc: &Document, page: usize) -> Result<PdfPage, Error> {
    PdfPage::try_from(doc.load_page(page as i32)?).map_err(|_| Error::NotPdf)
}

fn describe(w: &PdfWidget) -> Result<Field, Error> {
    let kind = match w.r#type()? {
        WidgetType::Text => FieldKind::Text,
        WidgetType::Checkbox => FieldKind::Checkbox,
        WidgetType::RadioButton => FieldKind::Radio,
        WidgetType::Combobox | WidgetType::Listbox => FieldKind::Choice,
        WidgetType::Button => FieldKind::Button,
        WidgetType::Signature => FieldKind::Signature,
        WidgetType::Unknown => FieldKind::Other,
    };
    let flags = w.field_flags()?;
    let mut value = w.value()?.unwrap_or_default();
    // A reset removes /V from boxes without a default; that reads as unchecked.
    if value.is_empty() && matches!(kind, FieldKind::Checkbox | FieldKind::Radio) {
        value = "Off".into();
    }
    Ok(Field {
        id: w.xref()?,
        kind,
        name: w.name()?.unwrap_or_default(),
        value,
        rect: w.annotation().bounds()?.into(),
        read_only: w.is_readonly()?,
        multiline: flags.contains(FieldFlags::MULTILINE),
        options: if kind == FieldKind::Choice {
            w.choice_options()?
        } else {
            Vec::new()
        },
    })
}

/// Whether the document has AcroForm fields at all, without loading any page.
pub fn has_fields(doc: &Document) -> Result<bool, Error> {
    let Ok(pdf) = PdfDocument::try_from(doc.clone()) else {
        return Ok(false);
    };
    let Some(form) = pdf.catalog()?.get_dict("AcroForm")? else {
        return Ok(false);
    };
    Ok(match form.get_dict("Fields")? {
        Some(fields) => fields.len()? > 0,
        None => false,
    })
}

/// The page's form widgets. Empty for documents that are not PDF.
pub fn list(doc: &Document, page: usize) -> Result<Vec<Field>, Error> {
    let Ok(page) = pdf_page(doc, page) else {
        return Ok(Vec::new());
    };
    page.widgets().map(|w| describe(&w)).collect()
}

pub fn edit(doc: &Document, page: usize, id: i32, edit: &FieldEdit) -> Result<(), Error> {
    let name = match edit {
        FieldEdit::Value(_) => "Fill in field",
        FieldEdit::Toggle => "Check box",
    };
    operation(doc, name, || {
        let mut pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
        prepare(&mut pdf)?;
        let mut page = pdf_page(doc, page)?;
        let mut widget = page.load_widget(id)?.ok_or(Error::NotFound)?;
        if widget.is_readonly()? {
            return Err(Error::Invalid("the field is read-only"));
        }
        let changed = match edit {
            FieldEdit::Value(text) => widget.set_value(&mut pdf, text, false)?,
            FieldEdit::Toggle => widget.toggle()?,
        };
        if !changed {
            return Err(Error::Invalid("the form did not accept that value"));
        }
        page.update()?;
        Ok(())
    })
}

/// Clears every field to its default value.
pub fn reset(doc: &Document) -> Result<(), Error> {
    operation(doc, "Reset form", || {
        let mut pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
        prepare(&mut pdf)?;
        for i in 0..doc.page_count()? {
            let mut page = pdf_page(doc, i as usize)?;
            for mut widget in page.widgets() {
                widget.reset(&mut pdf)?;
            }
            page.update()?;
        }
        Ok(())
    })
}

pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Each fillable field once, in page order: full name, value and kind.
fn values(doc: &Document) -> Result<Vec<(String, String, FieldKind)>, Error> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for page in 0..doc.page_count()? as usize {
        for field in list(doc, page)? {
            let fillable = matches!(
                field.kind,
                FieldKind::Text | FieldKind::Checkbox | FieldKind::Radio | FieldKind::Choice
            );
            // Radio buttons and repeated widgets share one field.
            if fillable && !field.name.is_empty() && seen.insert(field.name.clone()) {
                out.push((field.name, field.value, field.kind));
            }
        }
    }
    Ok(out)
}

/// The form's values as XFDF, the XML form-data format Acrobat reads. `file` names the PDF.
pub fn export_xfdf(doc: &Document, file: &str) -> Result<String, Error> {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <xfdf xmlns=\"http://ns.adobe.com/xfdf/\" xml:space=\"preserve\">\n",
    );
    out += &format!("<f href=\"{}\"/>\n<fields>\n", escape(file));
    for (name, value, _) in values(doc)? {
        out += &format!(
            "<field name=\"{}\"><value>{}</value></field>\n",
            escape(&name),
            escape(&value)
        );
    }
    out += "</fields>\n</xfdf>\n";
    Ok(out)
}

/// The form's values as FDF, the older form-data format written in PDF syntax. `file` names
/// the PDF.
pub fn export_fdf(doc: &Document, file: &str) -> Result<Vec<u8>, Error> {
    // Dotted names nest: "a.b" is the kid "b" of field "a".
    #[derive(Default)]
    struct Node {
        value: Option<(String, FieldKind)>,
        kids: Vec<(String, Node)>,
    }
    fn write(out: &mut Vec<u8>, kids: &[(String, Node)]) {
        out.push(b'[');
        for (name, node) in kids {
            out.extend_from_slice(b"<< /T ");
            out.extend_from_slice(&pdf_string(name));
            if let Some((value, kind)) = &node.value {
                out.extend_from_slice(b" /V ");
                // Checkboxes and radio buttons take the name of their on state.
                if matches!(kind, FieldKind::Checkbox | FieldKind::Radio) {
                    out.extend_from_slice(&pdf_name(value));
                } else {
                    out.extend_from_slice(&pdf_string(value));
                }
            }
            if !node.kids.is_empty() {
                out.extend_from_slice(b" /Kids ");
                write(out, &node.kids);
            }
            out.extend_from_slice(b" >>\n");
        }
        out.push(b']');
    }
    let mut root = Node::default();
    for (name, value, kind) in values(doc)? {
        let mut node = &mut root;
        for part in name.split('.') {
            let i = match node.kids.iter().position(|(n, _)| n == part) {
                Some(i) => i,
                None => {
                    node.kids.push((part.to_owned(), Node::default()));
                    node.kids.len() - 1
                }
            };
            node = &mut node.kids[i].1;
        }
        node.value = Some((value, kind));
    }
    let mut out = b"%FDF-1.2\n%\xe2\xe3\xcf\xd3\n1 0 obj\n<< /FDF << /F ".to_vec();
    out.extend_from_slice(&pdf_string(file));
    out.extend_from_slice(b" /Fields ");
    write(&mut out, &root.kids);
    out.extend_from_slice(b" >> >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%EOF\n");
    Ok(out)
}

/// A PDF string: literal for ASCII text, else UTF-16BE with a byte order mark, in hex.
fn pdf_string(text: &str) -> Vec<u8> {
    if text.is_ascii() {
        let mut out = vec![b'('];
        for b in text.bytes() {
            match b {
                b'(' | b')' | b'\\' => out.extend_from_slice(&[b'\\', b]),
                b'\r' => out.extend_from_slice(b"\\r"),
                b'\n' => out.extend_from_slice(b"\\n"),
                _ => out.push(b),
            }
        }
        out.push(b')');
        out
    } else {
        let mut out = String::from("<FEFF");
        for unit in text.encode_utf16() {
            out += &format!("{unit:04X}");
        }
        out.push('>');
        out.into_bytes()
    }
}

/// A PDF name, with `#xx` escapes for bytes outside the regular characters.
fn pdf_name(text: &str) -> Vec<u8> {
    let mut out = vec![b'/'];
    for b in text.bytes() {
        if (0x21..0x7f).contains(&b) && !b"#()<>[]{}/%".contains(&b) {
            out.push(b);
        } else {
            out.extend_from_slice(format!("#{b:02X}").as_bytes());
        }
    }
    out
}

/// Field values in XFDF, keyed by full name. Nested fields join their names with dots.
fn parse_xfdf(xml: &str) -> Result<Vec<(String, String)>, Error> {
    let tree = roxmltree::Document::parse(xml).map_err(|_| Error::Invalid("not an XFDF file"))?;
    let mut values = Vec::new();
    for node in tree.descendants().filter(|n| n.has_tag_name("field")) {
        let Some(value) = node.children().find(|n| n.has_tag_name("value")) else {
            continue;
        };
        let mut names: Vec<&str> = node
            .ancestors()
            .filter(|n| n.has_tag_name("field"))
            .filter_map(|n| n.attribute("name"))
            .collect();
        names.reverse();
        values.push((names.join("."), value.text().unwrap_or_default().to_owned()));
    }
    Ok(values)
}

/// Field values in FDF, keyed by full name. Nested fields join their names with dots.
fn parse_fdf(data: &[u8]) -> Result<Vec<(String, String)>, Error> {
    fn walk(
        fields: &PdfObject,
        prefix: &str,
        out: &mut Vec<(String, String)>,
    ) -> Result<(), Error> {
        for i in 0..fields.len()? as i32 {
            let Some(field) = fields.get_array(i)? else {
                continue;
            };
            let part = match field.get_dict("T")? {
                Some(t) => t.as_string()?,
                None => String::new(),
            };
            let name = match (prefix.is_empty(), part.is_empty()) {
                (true, _) => part,
                (false, true) => prefix.to_owned(),
                (false, false) => format!("{prefix}.{part}"),
            };
            if let Some(v) = field.get_dict("V")? {
                if v.is_name()? {
                    out.push((
                        name.clone(),
                        String::from_utf8_lossy(&v.as_name()?).into_owned(),
                    ));
                } else if v.is_string()? {
                    out.push((name.clone(), v.as_string()?));
                }
            }
            if let Some(kids) = field.get_dict("Kids")? {
                walk(&kids, &name, out)?;
            }
        }
        Ok(())
    }
    let bad = || Error::Invalid("not an FDF file");
    if !data.starts_with(b"%FDF") {
        return Err(bad());
    }
    // FDF is PDF syntax without a cross-reference table; MuPDF reads it by repairing it.
    let fdf = Document::from_bytes(data, "application/pdf").map_err(|_| bad())?;
    let fdf = PdfDocument::try_from(fdf).map_err(|_| bad())?;
    let fields = fdf
        .catalog()?
        .get_dict("FDF")?
        .and_then(|f| f.get_dict("Fields").ok().flatten())
        .ok_or_else(bad)?;
    let mut out = Vec::new();
    walk(&fields, "", &mut out)?;
    Ok(out)
}

/// Fills the form from XFDF as one undoable step. Returns how many fields took a value.
pub fn import_xfdf(doc: &Document, xml: &str) -> Result<usize, Error> {
    fill(doc, parse_xfdf(xml)?.into_iter().collect())
}

/// Fills the form from FDF as one undoable step. Returns how many fields took a value.
pub fn import_fdf(doc: &Document, data: &[u8]) -> Result<usize, Error> {
    fill(doc, parse_fdf(data)?.into_iter().collect())
}

fn fill(doc: &Document, values: std::collections::HashMap<String, String>) -> Result<usize, Error> {
    operation(doc, "Import form data", || {
        let mut pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
        prepare(&mut pdf)?;
        let mut filled = std::collections::HashSet::new();
        for i in 0..doc.page_count()? {
            let mut page = pdf_page(doc, i as usize)?;
            for mut widget in page.widgets() {
                let Some(name) = widget.name()? else { continue };
                let Some(value) = values.get(&name) else {
                    continue;
                };
                if widget.is_readonly()? {
                    continue;
                }
                if widget.set_value(&mut pdf, value, false)? {
                    filled.insert(name);
                }
            }
            page.update()?;
        }
        Ok(filled.len())
    })
}
