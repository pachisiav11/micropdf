//! Editing links and bookmarks. Links are Link annotations; bookmarks are written as a new
//! outline tree that keeps each existing item's dictionary, so actions, colours and styles
//! micropdf does not show stay as they were.

use std::collections::HashSet;

use mupdf::pdf::{LinkAction, PdfAction, PdfDestination, PdfDocument, PdfLink, PdfObject};
use mupdf::{DestinationKind, Document};

use crate::pages::pdf;
use crate::{DocId, Error, LinkTarget, OutlineItem, Rect};

fn action(target: &LinkTarget) -> PdfAction {
    match target {
        LinkTarget::Page { page, top } => PdfAction::GoTo(PdfDestination::Page {
            page: *page as u32,
            kind: DestinationKind::XYZ {
                left: None,
                top: *top,
                zoom: None,
            },
        }),
        LinkTarget::Uri(uri) => PdfAction::Uri(uri.clone()),
    }
}

/// The outline items in reading order, as MuPDF lists them.
fn items(pdf: &PdfDocument) -> Result<Vec<PdfObject>, Error> {
    fn walk(
        mut item: Option<PdfObject>,
        seen: &mut HashSet<i32>,
        out: &mut Vec<PdfObject>,
    ) -> Result<(), Error> {
        while let Some(i) = item {
            if !i.is_dict()? || !seen.insert(i.as_indirect()?) {
                break;
            }
            let first = i.get_dict("First")?;
            item = i.get_dict("Next")?;
            out.push(i);
            walk(first, seen, out)?;
        }
        Ok(())
    }
    let mut out = Vec::new();
    if let Some(root) = pdf.catalog()?.get_dict("Outlines")? {
        walk(root.get_dict("First")?, &mut HashSet::new(), &mut out)?;
    }
    Ok(out)
}

/// The destination of a new bookmark: `[page /XYZ null top null]` or `[page /Fit]`.
fn destination(pdf: &PdfDocument, page: usize, top: Option<f32>) -> Result<PdfObject, Error> {
    let mut dest = pdf.new_array()?;
    dest.array_push(pdf.find_page(page as i32)?)?;
    match top {
        Some(top) => {
            let p = pdf.load_pdf_page(page as i32)?;
            let to_pdf = p
                .ctm()?
                .invert()
                .ok_or(Error::Invalid("the page has no area"))?;
            let y = mupdf::Point::new(0.0, top).transform(&to_pdf).y;
            dest.array_push(PdfObject::new_name("XYZ")?)?;
            dest.array_push(PdfObject::new_null())?;
            dest.array_push(PdfObject::new_real(y)?)?;
            dest.array_push(PdfObject::new_null())?;
        }
        None => dest.array_push(PdfObject::new_name("Fit")?)?,
    }
    Ok(dest)
}

fn write_outline(doc: &Document, list: &[OutlineItem]) -> Result<(), Error> {
    let mut pdf = pdf(doc)?;
    let old = items(&pdf)?;
    let mut catalog = pdf.catalog()?;
    if list.is_empty() {
        catalog.dict_delete("Outlines")?;
        return Ok(());
    }
    // Each item at most one level below the one before it.
    let mut depth: Vec<usize> = Vec::with_capacity(list.len());
    for (i, item) in list.iter().enumerate() {
        depth.push(item.depth.min(if i == 0 { 0 } else { depth[i - 1] + 1 }));
    }
    let mut objs = Vec::with_capacity(list.len());
    let mut open = Vec::with_capacity(list.len());
    for item in list {
        let mut obj = match item.source.and_then(|s| old.get(s)) {
            Some(o) => o.clone(),
            None => {
                let mut d = pdf.new_dict()?;
                match &item.target {
                    Some(LinkTarget::Page { page, top }) => {
                        d.dict_put("Dest", destination(&pdf, *page, *top)?)?;
                    }
                    Some(LinkTarget::Uri(uri)) => {
                        let mut a = pdf.new_dict()?;
                        a.dict_put("S", PdfObject::new_name("URI")?)?;
                        a.dict_put("URI", PdfObject::new_string(uri)?)?;
                        d.dict_put("A", a)?;
                    }
                    None => {}
                }
                pdf.add_object(&d)?
            }
        };
        open.push(obj.get_dict("Count")?.map_or(Ok(0), |c| c.as_int())? >= 0);
        for key in ["Parent", "Prev", "Next", "First", "Last", "Count"] {
            obj.dict_delete(key)?;
        }
        obj.dict_put("Title", PdfObject::new_string(&item.title)?)?;
        objs.push(obj);
    }
    let mut root = pdf.new_dict()?;
    root.dict_put("Type", PdfObject::new_name("Outlines")?)?;
    let mut root = pdf.add_object(&root)?;
    // Children of each item, and of the root (the last entry).
    let n = list.len();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n + 1];
    let mut stack: Vec<usize> = Vec::new();
    for (i, &d) in depth.iter().enumerate() {
        stack.truncate(d);
        children[stack.last().copied().unwrap_or(n)].push(i);
        stack.push(i);
    }
    // Items shown under each one when it is open, deepest first.
    let mut shown = vec![0i32; n + 1];
    for i in (0..=n).rev() {
        shown[i] = children[i]
            .iter()
            .map(|&c| 1 + if open[c] { shown[c] } else { 0 })
            .sum();
    }
    for (i, kids) in children.iter().enumerate() {
        let mut parent = if i == n {
            root.clone()
        } else {
            objs[i].clone()
        };
        if let (Some(&first), Some(&last)) = (kids.first(), kids.last()) {
            parent.dict_put("First", objs[first].clone())?;
            parent.dict_put("Last", objs[last].clone())?;
            let count = if i == n || open[i] {
                shown[i]
            } else {
                -shown[i]
            };
            parent.dict_put("Count", PdfObject::new_int(count)?)?;
        }
        for (k, &c) in kids.iter().enumerate() {
            let mut child = objs[c].clone();
            child.dict_put("Parent", parent.clone())?;
            if k > 0 {
                child.dict_put("Prev", objs[kids[k - 1]].clone())?;
            }
            if let Some(&next) = kids.get(k + 1) {
                child.dict_put("Next", objs[next].clone())?;
            }
        }
    }
    root.dict_put("Count", PdfObject::new_int(shown[n])?)?;
    catalog.dict_put("Outlines", root)?;
    Ok(())
}

impl crate::Engine {
    /// Adds a link over `rect` on `page` to `target`, as one undo step.
    pub fn add_link(
        &self,
        doc: DocId,
        page: usize,
        rect: Rect,
        target: LinkTarget,
    ) -> Result<(), Error> {
        self.edit(doc, "Add link", move |d| {
            let mut pdf = pdf(d)?;
            let mut p = pdf.load_pdf_page(page as i32)?;
            let link = PdfLink {
                bounds: mupdf::Rect::new(rect.x0, rect.y0, rect.x1, rect.y1),
                action: LinkAction::Action(action(&target)),
            };
            p.add_links(&mut pdf, &[link])?;
            Ok(())
        })
    }

    /// Deletes the links over `rect` on `page`, as one undo step.
    pub fn delete_link(&self, doc: DocId, page: usize, rect: Rect) -> Result<(), Error> {
        self.edit(doc, "Delete link", move |d| {
            let pdf = pdf(d)?;
            let p = pdf.load_pdf_page(page as i32)?;
            let ctm = p.ctm()?;
            let Some(mut annots) = p.object().get_dict("Annots")? else {
                return Err(Error::NotFound);
            };
            let mut found = false;
            for i in (0..annots.len()? as i32).rev() {
                let Some(a) = annots.get_array(i)? else {
                    continue;
                };
                let link = a
                    .get_dict("Subtype")?
                    .is_some_and(|s| s.as_name().is_ok_and(|n| n == b"Link"));
                let Some(r) = a.get_dict("Rect")?.filter(|_| link) else {
                    continue;
                };
                let n = |k: i32| -> Result<f32, Error> {
                    Ok(r.get_array(k)?.map_or(Ok(0.0), |v| v.as_float())?)
                };
                let at: Rect = mupdf::Rect::new(n(0)?, n(1)?, n(2)?, n(3)?)
                    .transform(&ctm)
                    .into();
                if (at.x0 - rect.x0).abs() < 1.0
                    && (at.y0 - rect.y0).abs() < 1.0
                    && (at.x1 - rect.x1).abs() < 1.0
                    && (at.y1 - rect.y1).abs() < 1.0
                {
                    annots.array_delete(i)?;
                    found = true;
                }
            }
            if found { Ok(()) } else { Err(Error::NotFound) }
        })
    }

    /// Writes `items` as the document's bookmarks, as one undo step. Items with a `source`
    /// keep that bookmark's action and look; new ones go to their target.
    pub fn set_outline(&self, doc: DocId, items: Vec<OutlineItem>) -> Result<(), Error> {
        self.edit(doc, "Edit bookmarks", move |d| write_outline(d, &items))
    }
}
