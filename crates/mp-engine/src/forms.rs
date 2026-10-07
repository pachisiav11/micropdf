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

#[derive(Debug, Clone, PartialEq)]
pub enum FieldEdit {
    /// New text for a text field, or the chosen option of a list or combo box.
    Value(String),
    /// Checks or unchecks a checkbox or radio button.
    Toggle,
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
            return Err(Error::Invalid("the field did not take the value"));
        }
        page.update()?;
        Ok(())
    })
}

/// Clears every field to its default value.
pub fn reset(doc: &Document) -> Result<(), Error> {
    operation(doc, "Reset form", || {
        let mut pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
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
