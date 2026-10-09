//! Prepare Form: new fields of every kind, their properties and actions, the tab order, and
//! fields found where a page asks to be filled in: runs of underscores, empty boxes and empty
//! table cells.

use std::collections::HashSet;

use mupdf::Document;
use mupdf::drawing::DrawingItem;
use mupdf::pdf::{PdfDocument, PdfObject, PdfPage, PdfWidget, WidgetType};
use mupdf::text_page::TextPageFlags;

use crate::forms::on_state;
use crate::pages::pdf;
use crate::{DocId, Error, FieldKind, Rect};

/// A field Prepare Form adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewField {
    Text,
    Checkbox,
    Radio,
    Dropdown,
    List,
    Button,
    Signature,
}

/// What a click on a field does.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum FieldAction {
    #[default]
    None,
    Uri(String),
    Reset,
    Submit(String),
    Script(String),
}

/// A field's properties as Prepare Form shows and changes them.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldProps {
    pub kind: FieldKind,
    pub name: String,
    pub tooltip: String,
    pub required: bool,
    pub read_only: bool,
    /// A text field takes several lines.
    pub multiline: bool,
    /// A list or dropdown's options, and whether it is a list rather than a dropdown.
    pub options: Vec<String>,
    pub list: bool,
    /// The value a checkbox or radio button has when on.
    pub export: String,
    /// A button's label.
    pub label: String,
    pub action: FieldAction,
    /// JavaScript that computes a text field's value from others.
    pub calculate: String,
}

const READ_ONLY: i32 = 1;
const REQUIRED: i32 = 1 << 1;
const MULTILINE: i32 = 1 << 12;
const NO_TOGGLE_OFF: i32 = 1 << 14;
const RADIO: i32 = 1 << 15;
const PUSH_BUTTON: i32 = 1 << 16;
const COMBO: i32 = 1 << 17;

fn string(obj: &PdfObject, key: &str) -> Result<String, Error> {
    Ok(match obj.get_dict(key)? {
        Some(v) if v.is_string()? => v.as_string().unwrap_or_default(),
        _ => String::new(),
    })
}

fn put_string(obj: &mut PdfObject, key: &str, value: &str) -> Result<(), Error> {
    if value.is_empty() {
        obj.dict_delete(key)?;
    } else {
        obj.dict_put(key, PdfObject::new_string(value)?)?;
    }
    Ok(())
}

/// The field a widget belongs to: itself when it carries the name, else its parent.
fn field_of(widget: &PdfObject) -> Result<PdfObject, Error> {
    if widget.get_dict("T")?.is_none()
        && let Some(parent) = widget.get_dict("Parent")?
    {
        return Ok(parent);
    }
    Ok(widget.clone())
}

fn flags(field: &PdfObject) -> Result<i32, Error> {
    Ok(match field.get_dict_inheritable("Ff")? {
        Some(f) => f.as_int()?,
        None => 0,
    })
}

/// Every field name in the document, at any level.
fn names(pdf: &PdfDocument) -> Result<HashSet<String>, Error> {
    fn walk(list: Option<PdfObject>, depth: usize, out: &mut HashSet<String>) -> Result<(), Error> {
        let Some(list) = list.filter(|_| depth < 32) else {
            return Ok(());
        };
        for i in 0..list.len()? as i32 {
            if let Some(f) = list.get_array(i)? {
                out.insert(string(&f, "T")?);
                walk(f.get_dict("Kids")?, depth + 1, out)?;
            }
        }
        Ok(())
    }
    let mut out = HashSet::new();
    if let Some(form) = pdf.catalog()?.get_dict("AcroForm")? {
        walk(form.get_dict("Fields")?, 0, &mut out)?;
    }
    Ok(out)
}

/// Adds a field; a radio button joins the group of radio button `group` when given.
fn add(
    pdf: &mut PdfDocument,
    page: &mut PdfPage,
    kind: NewField,
    rect: Rect,
    group: Option<i32>,
) -> Result<i32, Error> {
    let (prefix, ft, ff) = match kind {
        NewField::Text => ("Text", "Tx", 0),
        NewField::Checkbox => ("Check Box", "Btn", 0),
        NewField::Radio => ("Group", "Btn", RADIO | NO_TOGGLE_OFF),
        NewField::Dropdown => ("Dropdown", "Ch", COMBO),
        NewField::List => ("List Box", "Ch", 0),
        NewField::Button => ("Button", "Btn", PUSH_BUTTON),
        NewField::Signature => ("Signature", "Sig", 0),
    };
    let taken = names(pdf)?;
    let name = (1..)
        .map(|n| format!("{prefix}{n}"))
        .find(|n| !taken.contains(n))
        .expect("some name is free");
    // MuPDF makes a widget, and lists it in the form, only as a signature field.
    let mut widget = page.add_signature_widget(&name)?;
    let mut obj = widget.annotation().object();
    obj.dict_put("FT", PdfObject::new_name(ft)?)?;
    if kind != NewField::Signature {
        obj.dict_delete("Lock")?;
    }
    let mut mk = pdf.new_dict()?;
    let mut gray = pdf.new_array()?;
    for _ in 0..3 {
        gray.array_push(PdfObject::new_real(0.6)?)?;
    }
    mk.dict_put("BC", gray)?;
    if kind == NewField::Button {
        let mut white = pdf.new_array()?;
        white.array_push(PdfObject::new_real(0.9)?)?;
        mk.dict_put("BG", white)?;
        mk.dict_put("CA", PdfObject::new_string("Button")?)?;
    }
    obj.dict_put("MK", mk)?;
    if ff != 0 {
        obj.dict_put("Ff", PdfObject::new_int(ff)?)?;
    }
    if kind == NewField::Radio {
        // A group is a parent field; each button is a kid with its own value.
        let joined = match group {
            Some(id) => Some(field_of(&widget_at(page, id)?.annotation().object())?)
                .filter(|p| p.get_dict("Kids").is_ok_and(|k| k.is_some())),
            None => None,
        };
        let parent = match &joined {
            Some(parent) => parent.clone(),
            None => {
                let mut parent = pdf.new_dict()?;
                parent.dict_put("FT", PdfObject::new_name("Btn")?)?;
                parent.dict_put("Ff", PdfObject::new_int(ff)?)?;
                parent.dict_put("T", PdfObject::new_string(&name)?)?;
                parent.dict_put("Kids", pdf.new_array()?)?;
                pdf.add_object(&parent)?
            }
        };
        let mut kids = parent.get_dict("Kids")?.ok_or(Error::NotFound)?;
        let mut states = HashSet::new();
        for i in 0..kids.len()? as i32 {
            let n = kids
                .get_array(i)?
                .and_then(|k| k.get_dict("AP").ok().flatten());
            if let Some(n) = n.and_then(|ap| ap.get_dict("N").ok().flatten()) {
                for j in 0..n.dict_len()? as i32 {
                    if let Some(key) = n.get_dict_key(j)? {
                        states.insert(String::from_utf8_lossy(&key.as_name()?).into_owned());
                    }
                }
            }
        }
        let export = (1..)
            .map(|n| format!("Choice{n}"))
            .find(|s| !states.contains(s))
            .expect("some value is free");
        kids.array_push(obj.clone())?;
        for key in ["T", "FT", "Ff"] {
            obj.dict_delete(key)?;
        }
        obj.dict_put("Parent", parent.clone())?;
        obj.dict_put("AS", PdfObject::new_name(&export)?)?;
        // The form lists the group, not its buttons.
        let mut fields = pdf
            .catalog()?
            .get_dict("AcroForm")?
            .and_then(|f| f.get_dict("Fields").ok().flatten())
            .ok_or(Error::NotFound)?;
        let me = obj.as_indirect()?;
        for i in (0..fields.len()? as i32).rev() {
            if fields
                .get_array(i)?
                .is_some_and(|f| f.as_indirect().ok() == Some(me))
            {
                if joined.is_some() {
                    fields.array_delete(i)?;
                } else {
                    fields.array_put(i, parent.clone())?;
                }
            }
        }
    }
    // A new Rect asks MuPDF to draw the field's appearance.
    widget
        .annotation_mut()
        .set_rect(mupdf::Rect::new(rect.x0, rect.y0, rect.x1, rect.y1))?;
    widget.update()?;
    if kind == NewField::Radio {
        obj.dict_put("AS", PdfObject::new_name("Off")?)?;
    }
    Ok(widget.xref()?)
}

fn widget_at(page: &PdfPage, id: i32) -> Result<PdfWidget, Error> {
    page.load_widget(id)?.ok_or(Error::NotFound)
}

fn action_of(obj: &PdfObject) -> Result<FieldAction, Error> {
    let Some(a) = obj.get_dict("A")? else {
        return Ok(FieldAction::None);
    };
    let kind = a.get_dict("S")?.map_or(Ok(Vec::new()), |s| s.as_name())?;
    Ok(match kind.as_slice() {
        b"URI" => FieldAction::Uri(string(&a, "URI")?),
        b"ResetForm" => FieldAction::Reset,
        b"SubmitForm" => FieldAction::Submit(match a.get_dict("F")? {
            Some(f) if f.is_dict()? => string(&f, "F")?,
            Some(f) => f.as_string().unwrap_or_default(),
            None => String::new(),
        }),
        b"JavaScript" => FieldAction::Script(script(&a)?),
        _ => FieldAction::None,
    })
}

/// A JavaScript action's code, from a string or a stream.
fn script(action: &PdfObject) -> Result<String, Error> {
    Ok(match action.get_dict("JS")? {
        Some(js) if js.is_stream()? => String::from_utf8_lossy(&js.read_stream()?).into_owned(),
        Some(js) => js.as_string().unwrap_or_default(),
        None => String::new(),
    })
}

fn props(page: &PdfPage, id: i32) -> Result<FieldProps, Error> {
    let w = widget_at(page, id)?;
    let obj = w.annotation().object();
    let field = field_of(&obj)?;
    let ff = flags(&obj)?;
    let kind = match w.r#type()? {
        WidgetType::Text => FieldKind::Text,
        WidgetType::Checkbox => FieldKind::Checkbox,
        WidgetType::RadioButton => FieldKind::Radio,
        WidgetType::Combobox | WidgetType::Listbox => FieldKind::Choice,
        WidgetType::Button => FieldKind::Button,
        WidgetType::Signature => FieldKind::Signature,
        WidgetType::Unknown => FieldKind::Other,
    };
    let calculate = match field
        .get_dict("AA")?
        .and_then(|aa| aa.get_dict("C").ok().flatten())
    {
        Some(c) => script(&c)?,
        None => String::new(),
    };
    Ok(FieldProps {
        kind,
        name: string(&field, "T")?,
        tooltip: string(&field, "TU")?,
        required: ff & REQUIRED != 0,
        read_only: ff & READ_ONLY != 0,
        multiline: ff & MULTILINE != 0,
        options: if kind == FieldKind::Choice {
            w.choice_options()?
        } else {
            Vec::new()
        },
        list: w.r#type()? == WidgetType::Listbox,
        export: on_state(&w)?.unwrap_or_default(),
        label: match obj.get_dict("MK")? {
            Some(mk) => string(&mk, "CA")?,
            None => String::new(),
        },
        action: action_of(&obj)?,
        calculate,
    })
}

fn action_dict(pdf: &PdfDocument, action: &FieldAction) -> Result<Option<PdfObject>, Error> {
    let mut a = pdf.new_dict()?;
    let (kind, key, value) = match action {
        FieldAction::None => return Ok(None),
        FieldAction::Uri(uri) => ("URI", "URI", uri),
        FieldAction::Reset => ("ResetForm", "", &String::new()),
        FieldAction::Submit(url) => ("SubmitForm", "", url),
        FieldAction::Script(js) => ("JavaScript", "JS", js),
    };
    a.dict_put("S", PdfObject::new_name(kind)?)?;
    if !key.is_empty() {
        a.dict_put(key, PdfObject::new_string(value)?)?;
    }
    if let FieldAction::Submit(url) = action {
        let mut spec = pdf.new_dict()?;
        spec.dict_put("FS", PdfObject::new_name("URL")?)?;
        spec.dict_put("F", PdfObject::new_string(url)?)?;
        a.dict_put("F", spec)?;
        // Sent as HTML form data.
        a.dict_put("Flags", PdfObject::new_int(4)?)?;
    }
    Ok(Some(a))
}

/// Renames the on state of a checkbox or radio button from `old` to `new` in its appearances,
/// its state and its field's value.
fn rename_state(
    obj: &mut PdfObject,
    field: &mut PdfObject,
    old: &str,
    new: &str,
) -> Result<(), Error> {
    for which in ["N", "D"] {
        if let Some(mut states) = obj
            .get_dict("AP")?
            .and_then(|ap| ap.get_dict(which).ok().flatten())
            && states.is_dict()?
            && let Some(look) = states.get_dict(old)?
        {
            states.dict_put(new, look)?;
            states.dict_delete(old)?;
        }
    }
    let new_name = PdfObject::new_name(new)?;
    for (o, key) in [(obj, "AS"), (field, "V")] {
        if o.get_dict(key)?
            .is_some_and(|v| v.as_name().is_ok_and(|n| n == old.as_bytes()))
        {
            o.dict_put(key, new_name.clone())?;
        }
    }
    Ok(())
}

fn set_props(pdf: &mut PdfDocument, page: &PdfPage, id: i32, p: &FieldProps) -> Result<(), Error> {
    let mut w = widget_at(page, id)?;
    let mut obj = w.annotation().object();
    let mut field = field_of(&obj)?;
    if p.name.trim().is_empty() {
        return Err(Error::Invalid("a field needs a name"));
    }
    put_string(&mut field, "T", p.name.trim())?;
    put_string(&mut field, "TU", &p.tooltip)?;
    let mut ff = flags(&obj)? & !(READ_ONLY | REQUIRED);
    ff |= if p.read_only { READ_ONLY } else { 0 } | if p.required { REQUIRED } else { 0 };
    match p.kind {
        FieldKind::Text => ff = (ff & !MULTILINE) | if p.multiline { MULTILINE } else { 0 },
        FieldKind::Choice => {
            ff = (ff & !COMBO) | if p.list { 0 } else { COMBO };
            let mut opt = pdf.new_array()?;
            for o in p.options.iter().filter(|o| !o.trim().is_empty()) {
                opt.array_push(PdfObject::new_string(o.trim())?)?;
            }
            field.dict_put("Opt", opt)?;
            let value = string(&field, "V")?;
            if !value.is_empty() && !p.options.iter().any(|o| o.trim() == value) {
                field.dict_delete("V")?;
            }
        }
        FieldKind::Checkbox | FieldKind::Radio => {
            let old = on_state(&w)?.unwrap_or_else(|| "Yes".into());
            let new = p.export.trim();
            if !new.is_empty() && new != old && new != "Off" {
                rename_state(&mut obj, &mut field, &old, new)?;
            }
        }
        FieldKind::Button => {
            if let Some(mut mk) = obj.get_dict("MK")? {
                put_string(&mut mk, "CA", &p.label)?;
            }
        }
        _ => {}
    }
    field.dict_put("Ff", PdfObject::new_int(ff)?)?;
    match action_dict(pdf, &p.action)? {
        Some(a) => obj.dict_put("A", a)?,
        None => obj.dict_delete("A")?,
    }
    set_calculation(pdf, &mut field, &p.calculate)?;
    let rect = w.annotation().rect()?;
    w.annotation_mut().set_rect(rect)?;
    w.update()?;
    Ok(())
}

/// Sets `field`'s calculate script, and lists it in the form's calculation order or takes it
/// off.
fn set_calculation(pdf: &PdfDocument, field: &mut PdfObject, js: &str) -> Result<(), Error> {
    let mut aa = match field.get_dict("AA")? {
        Some(aa) => aa,
        None if js.trim().is_empty() => return Ok(()),
        None => {
            let aa = pdf.new_dict()?;
            field.dict_put("AA", aa)?;
            field.get_dict("AA")?.ok_or(Error::NotFound)?
        }
    };
    let Some(mut form) = pdf.catalog()?.get_dict("AcroForm")? else {
        return Ok(());
    };
    let mut order = match form.get_dict("CO")? {
        Some(co) => co,
        None => {
            form.dict_put("CO", pdf.new_array()?)?;
            form.get_dict("CO")?.ok_or(Error::NotFound)?
        }
    };
    let me = field.as_indirect()?;
    for i in (0..order.len()? as i32).rev() {
        if order
            .get_array(i)?
            .is_some_and(|f| f.as_indirect().ok() == Some(me))
        {
            order.array_delete(i)?;
        }
    }
    if js.trim().is_empty() {
        aa.dict_delete("C")?;
        return Ok(());
    }
    let mut c = pdf.new_dict()?;
    c.dict_put("S", PdfObject::new_name("JavaScript")?)?;
    c.dict_put("JS", PdfObject::new_string(js)?)?;
    aa.dict_put("C", c)?;
    order.array_push(field.clone())?;
    Ok(())
}

/// Puts the page's fields in reading order, by rows or by columns, for Tab to follow.
fn order(pdf: &PdfDocument, page: &PdfPage, columns: bool) -> Result<(), Error> {
    let mut obj = page.object();
    let Some(annots) = obj.get_dict("Annots")? else {
        return Ok(());
    };
    let ctm = page.ctm()?;
    let mut others = Vec::new();
    let mut widgets = Vec::new();
    for i in 0..annots.len()? as i32 {
        let Some(a) = annots.get_array(i)? else {
            continue;
        };
        let widget = a
            .get_dict("Subtype")?
            .is_some_and(|s| s.as_name().is_ok_and(|n| n == b"Widget"));
        match a.get_dict("Rect")?.filter(|_| widget) {
            Some(r) => {
                let n = |k: i32| -> Result<f32, Error> {
                    Ok(r.get_array(k)?.map_or(Ok(0.0), |v| v.as_float())?)
                };
                let at = mupdf::Rect::new(n(0)?, n(1)?, n(2)?, n(3)?).transform(&ctm);
                // Rows a few points apart count as one.
                let (across, down) = ((at.x0 / 6.0).round(), (at.y0 / 6.0).round());
                let key = if columns {
                    (across, at.y0)
                } else {
                    (down, at.x0)
                };
                widgets.push((key, a));
            }
            None => others.push(a),
        }
    }
    widgets.sort_by(|a, b| a.0.0.total_cmp(&b.0.0).then(a.0.1.total_cmp(&b.0.1)));
    let mut list = pdf.new_array()?;
    for a in others.into_iter().chain(widgets.into_iter().map(|w| w.1)) {
        list.array_push(a)?;
    }
    obj.dict_put("Annots", list)?;
    obj.dict_put(
        "Tabs",
        PdfObject::new_name(if columns { "C" } else { "R" })?,
    )?;
    Ok(())
}

/// Places on `page` that look like they are for filling in, and the field each wants.
fn places(page: &PdfPage) -> Result<Vec<(Rect, NewField)>, Error> {
    let text = page.to_text_page(TextPageFlags::empty())?;
    let mut found = Vec::new();
    let mut letters: Vec<Rect> = Vec::new();
    for block in text.blocks() {
        for line in block.lines() {
            let mut run: Option<(Rect, f32, f32)> = None;
            let end = |run: &mut Option<(Rect, f32, f32)>, found: &mut Vec<_>| {
                if let Some((r, base, size)) = run.take()
                    && r.width() > size * 1.2
                {
                    let rect = Rect {
                        x0: r.x0,
                        y0: base - size * 1.1,
                        x1: r.x1,
                        y1: base + size * 0.25,
                    };
                    found.push((rect, NewField::Text));
                }
            };
            for c in line.chars() {
                let r: Rect = c.quad().into();
                if c.char() == Some('_') {
                    let (base, size) = (c.origin().y, c.size());
                    run = Some(match run {
                        Some((s, ..)) => (s.union(&r), base, size),
                        None => (r, base, size),
                    });
                } else {
                    end(&mut run, &mut found);
                    if c.char().is_some_and(|ch| !ch.is_whitespace()) {
                        letters.push(r);
                    }
                }
            }
            end(&mut run, &mut found);
        }
    }
    // Ruled lines and box edges: (where across, from, to).
    let mut rows: Vec<(f32, f32, f32)> = Vec::new();
    let mut cols: Vec<(f32, f32, f32)> = Vec::new();
    let mut edge = |a: (f32, f32), b: (f32, f32)| {
        if (a.1 - b.1).abs() < 1.0 && (a.0 - b.0).abs() > 4.0 {
            rows.push(((a.1 + b.1) / 2.0, a.0.min(b.0), a.0.max(b.0)));
        } else if (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() > 4.0 {
            cols.push(((a.0 + b.0) / 2.0, a.1.min(b.1), a.1.max(b.1)));
        }
    };
    for d in page.drawings()? {
        for item in d.items {
            match item {
                DrawingItem::Line(a, b) => edge((a.x, a.y), (b.x, b.y)),
                DrawingItem::Rect { rect: r, .. } if r.height() < 2.5 => {
                    let y = (r.y0 + r.y1) / 2.0;
                    edge((r.x0, y), (r.x1, y));
                }
                DrawingItem::Rect { rect: r, .. } if r.width() < 2.5 => {
                    let x = (r.x0 + r.x1) / 2.0;
                    edge((x, r.y0), (x, r.y1));
                }
                DrawingItem::Rect { rect: r, .. } => {
                    edge((r.x0, r.y0), (r.x1, r.y0));
                    edge((r.x0, r.y1), (r.x1, r.y1));
                    edge((r.x0, r.y0), (r.x0, r.y1));
                    edge((r.x1, r.y0), (r.x1, r.y1));
                }
                _ => {}
            }
        }
    }
    if rows.len() + cols.len() > 4000 {
        return Ok(found);
    }
    let levels = |lines: &[(f32, f32, f32)]| {
        let mut v: Vec<f32> = lines.iter().map(|l| l.0).collect();
        v.sort_by(f32::total_cmp);
        v.dedup_by(|a, b| (*a - *b).abs() < 1.5);
        v
    };
    let covers = |lines: &[(f32, f32, f32)], at: f32, from: f32, to: f32| {
        lines
            .iter()
            .any(|l| (l.0 - at).abs() < 1.5 && l.1 <= from + 1.5 && l.2 >= to - 1.5)
    };
    let (ys, xs) = (levels(&rows), levels(&cols));
    for pair in ys.windows(2) {
        let (top, bottom) = (pair[0], pair[1]);
        if !(8.0..=80.0).contains(&(bottom - top)) {
            continue;
        }
        // The upright lines that span this row, then each cell between two of them.
        let sides: Vec<f32> = xs
            .iter()
            .copied()
            .filter(|&x| covers(&cols, x, top, bottom))
            .collect();
        for side in sides.windows(2) {
            let (left, right) = (side[0], side[1]);
            if right - left < 8.0
                || !covers(&rows, top, left, right)
                || !covers(&rows, bottom, left, right)
            {
                continue;
            }
            let cell = Rect {
                x0: left,
                y0: top,
                x1: right,
                y1: bottom,
            };
            if letters
                .iter()
                .any(|l| cell.contains((l.x0 + l.x1) / 2.0, (l.y0 + l.y1) / 2.0))
            {
                continue;
            }
            let (w, h) = (cell.width(), cell.height());
            let kind = if w <= 20.0 && (w - h).abs() < 3.0 {
                NewField::Checkbox
            } else {
                NewField::Text
            };
            let inset = if kind == NewField::Checkbox { 0.0 } else { 1.0 };
            found.push((
                Rect {
                    x0: left + inset,
                    y0: top + inset,
                    x1: right - inset,
                    y1: bottom - inset,
                },
                kind,
            ));
        }
    }
    Ok(found)
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    let w = a.x1.min(b.x1) - a.x0.max(b.x0);
    let h = a.y1.min(b.y1) - a.y0.max(b.y0);
    w > 0.0 && h > 0.0 && w * h > 0.3 * (a.width() * a.height()).min(b.width() * b.height())
}

fn detect(doc: &Document, pages: &[usize]) -> Result<usize, Error> {
    let mut pdf = pdf(doc)?;
    let mut added = 0;
    for &p in pages {
        let mut page = pdf.load_pdf_page(p as i32)?;
        let mut taken: Vec<Rect> = page
            .widgets()
            .filter_map(|w| w.annotation().bounds().ok().map(Rect::from))
            .collect();
        for (rect, kind) in places(&page)? {
            if taken.iter().any(|t| overlaps(t, &rect)) {
                continue;
            }
            add(&mut pdf, &mut page, kind, rect, None)?;
            taken.push(rect);
            added += 1;
        }
    }
    Ok(added)
}

impl crate::Engine {
    /// Adds a field of `kind` in `rect` on `page`, named after its kind, as one undo step;
    /// returns its widget's id. A radio button joins the group of radio button `group` when
    /// given, else starts a group.
    pub fn add_field(
        &self,
        doc: DocId,
        page: usize,
        kind: NewField,
        rect: Rect,
        group: Option<i32>,
    ) -> Result<i32, Error> {
        self.edit(doc, "Add field", move |d| {
            let mut pdf = pdf(d)?;
            let mut page = pdf.load_pdf_page(page as i32)?;
            add(&mut pdf, &mut page, kind, rect, group)
        })
    }

    pub fn field_props(&self, doc: DocId, page: usize, id: i32) -> Result<FieldProps, Error> {
        self.read(doc, move |d, _| {
            props(&pdf(d)?.load_pdf_page(page as i32)?, id)
        })
    }

    /// Changes a field's properties and action, as one undo step.
    pub fn set_field_props(
        &self,
        doc: DocId,
        page: usize,
        id: i32,
        props: FieldProps,
    ) -> Result<(), Error> {
        self.edit(doc, "Field properties", move |d| {
            let mut pdf = pdf(d)?;
            let page = pdf.load_pdf_page(page as i32)?;
            set_props(&mut pdf, &page, id, &props)
        })
    }

    /// Moves or resizes a field's widget to `rect`, as one undo step.
    pub fn move_field(&self, doc: DocId, page: usize, id: i32, rect: Rect) -> Result<(), Error> {
        self.edit(doc, "Move field", move |d| {
            let page = pdf(d)?.load_pdf_page(page as i32)?;
            let mut w = widget_at(&page, id)?;
            w.annotation_mut()
                .set_rect(mupdf::Rect::new(rect.x0, rect.y0, rect.x1, rect.y1))?;
            w.update()?;
            Ok(())
        })
    }

    /// Deletes a field's widget, and the field with it, as one undo step.
    pub fn delete_field(&self, doc: DocId, page: usize, id: i32) -> Result<(), Error> {
        self.remove(doc, "Delete field", move |d| {
            let mut page = pdf(d)?.load_pdf_page(page as i32)?;
            let w = widget_at(&page, id)?;
            page.delete_widget(w)?;
            Ok(())
        })
    }

    /// Orders the fields of `pages` for Tab by rows, or by columns, as one undo step.
    pub fn order_fields(&self, doc: DocId, pages: Vec<usize>, columns: bool) -> Result<(), Error> {
        self.edit(doc, "Order fields", move |d| {
            let pdf = pdf(d)?;
            for p in pages {
                order(&pdf, &pdf.load_pdf_page(p as i32)?, columns)?;
            }
            Ok(())
        })
    }

    /// Adds fields where `pages` ask to be filled in: text fields over runs of underscores,
    /// empty boxes and empty table cells, and checkboxes in small empty squares. Returns how
    /// many it added, as one undo step.
    pub fn detect_fields(&self, doc: DocId, pages: Vec<usize>) -> Result<usize, Error> {
        self.edit(doc, "Detect fields", move |d| detect(d, &pages))
    }
}
