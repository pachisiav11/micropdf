//! Form fields: list them, fill them in and reset them. Each change is one undoable step.

use mupdf::Document;
use mupdf::pdf::{FieldFlags, PdfDocument, PdfPage, PdfWidget, WidgetType};

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

fn escape(text: &str) -> String {
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

/// The form's values as XFDF, the XML form-data format Acrobat reads. `file` names the PDF.
pub fn export_xfdf(doc: &Document, file: &str) -> Result<String, Error> {
    let mut seen = std::collections::HashSet::new();
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <xfdf xmlns=\"http://ns.adobe.com/xfdf/\" xml:space=\"preserve\">\n",
    );
    out += &format!("<f href=\"{}\"/>\n<fields>\n", escape(file));
    for page in 0..doc.page_count()? as usize {
        for field in list(doc, page)? {
            let fillable = matches!(
                field.kind,
                FieldKind::Text | FieldKind::Checkbox | FieldKind::Radio | FieldKind::Choice
            );
            // Radio buttons and repeated widgets share one field.
            if fillable && !field.name.is_empty() && seen.insert(field.name.clone()) {
                out += &format!(
                    "<field name=\"{}\"><value>{}</value></field>\n",
                    escape(&field.name),
                    escape(&field.value)
                );
            }
        }
    }
    out += "</fields>\n</xfdf>\n";
    Ok(out)
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

/// Fills the form from XFDF as one undoable step. Returns how many fields took a value.
pub fn import_xfdf(doc: &Document, xml: &str) -> Result<usize, Error> {
    let values: std::collections::HashMap<String, String> = parse_xfdf(xml)?.into_iter().collect();
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
