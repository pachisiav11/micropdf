//! Comments as XFDF, the XML format Acrobat uses to export and import review comments.
//!
//! Both directions work on the annotations' PDF dictionaries, in PDF page space, so the numbers
//! pass through unchanged. A stamp carries its appearance stream, so a signature keeps its look,
//! and a file attachment carries the file. Links, form widgets and popups are not exported.

use std::collections::HashMap;

use base64::prelude::{BASE64_STANDARD, Engine as _};
use mupdf::pdf::{EmbeddedFileOptions, PdfAnnotationType, PdfDocument, PdfObject, PdfPage};
use mupdf::{Buffer, Document};

use crate::Error;
use crate::annots::{name, operation, string};
use crate::forms::escape;

/// XFDF element names, the PDF subtypes they stand for, and MuPDF's annotation types.
const KINDS: [(&str, &str, PdfAnnotationType); 14] = [
    ("text", "Text", PdfAnnotationType::Text),
    ("freetext", "FreeText", PdfAnnotationType::FreeText),
    ("highlight", "Highlight", PdfAnnotationType::Highlight),
    ("underline", "Underline", PdfAnnotationType::Underline),
    ("strikeout", "StrikeOut", PdfAnnotationType::StrikeOut),
    ("squiggly", "Squiggly", PdfAnnotationType::Squiggly),
    ("ink", "Ink", PdfAnnotationType::Ink),
    ("square", "Square", PdfAnnotationType::Square),
    ("circle", "Circle", PdfAnnotationType::Circle),
    ("line", "Line", PdfAnnotationType::Line),
    ("polygon", "Polygon", PdfAnnotationType::Polygon),
    ("polyline", "PolyLine", PdfAnnotationType::PolyLine),
    ("stamp", "Stamp", PdfAnnotationType::Stamp),
    (
        "fileattachment",
        "FileAttachment",
        PdfAnnotationType::FileAttachment,
    ),
];

/// How deep an appearance's objects may nest before the rest is left out.
const MAX_DEPTH: usize = 32;

/// The annotation flags in bit order, as XFDF names them.
const FLAGS: [&str; 10] = [
    "invisible",
    "hidden",
    "print",
    "nozoom",
    "norotate",
    "noview",
    "readonly",
    "locked",
    "togglenoview",
    "lockedcontents",
];

fn get(obj: &PdfObject, key: &str) -> Result<Option<PdfObject>, Error> {
    Ok(obj.get_dict(key)?)
}

fn numbers(obj: &PdfObject) -> Result<Vec<f32>, Error> {
    let mut out = Vec::new();
    if obj.is_array()? {
        for item in obj.array_iter()? {
            let item = item?;
            if item.is_number()? {
                out.push(item.as_float()?);
            }
        }
    }
    Ok(out)
}

/// A number without trailing zeros: 12, 12.5, 12.3456.
fn num(n: f32) -> String {
    let s = format!("{n:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn join(values: &[f32], sep: &str) -> String {
    values.iter().map(|&v| num(v)).collect::<Vec<_>>().join(sep)
}

/// Points as XFDF writes them in gestures and vertices: "x,y;x,y".
fn points(values: &[f32]) -> String {
    values
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[x, y]| format!("{},{}", num(*x), num(*y)))
        .collect::<Vec<_>>()
        .join(";")
}

fn hex(color: &[f32]) -> Option<String> {
    let [r, g, b] = match *color {
        [g] => [g; 3],
        [r, g, b] => [r, g, b],
        [c, m, y, k] => [
            (1.0 - c) * (1.0 - k),
            (1.0 - m) * (1.0 - k),
            (1.0 - y) * (1.0 - k),
        ],
        _ => return None,
    };
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(format!("#{:02X}{:02X}{:02X}", byte(r), byte(g), byte(b)))
}

fn border_width(obj: &PdfObject) -> Result<Option<f32>, Error> {
    if let Some(bs) = get(obj, "BS")?
        && let Some(w) = get(&bs, "W")?
        && w.is_number()?
    {
        return Ok(Some(w.as_float()?));
    }
    Ok(None)
}

/// The document's comments as XFDF, and how many there are. `file` names the PDF.
pub fn export(doc: &Document, file: &str) -> Result<(String, usize), Error> {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <xfdf xmlns=\"http://ns.adobe.com/xfdf/\" xml:space=\"preserve\">\n<annots>\n",
    );
    let mut count = 0;
    for page_no in 0..doc.page_count()? {
        let Ok(page) = PdfPage::try_from(doc.load_page(page_no)?) else {
            break;
        };
        for annot in page.annotations() {
            let obj = annot.object();
            let Some(subtype) = name(&obj, "Subtype")? else {
                continue;
            };
            let Some(&(tag, _, _)) = KINDS.iter().find(|k| k.1 == subtype) else {
                continue;
            };
            let mut attrs = vec![("page", page_no.to_string())];
            let mut children = String::new();
            if let Some(r) = get(&obj, "Rect")? {
                let r = numbers(&r)?;
                if let [x0, y0, x1, y1] = r[..] {
                    let rect = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
                    attrs.push(("rect", join(&rect, ",")));
                }
            }
            // The box inside the rectangle, for callouts and cloudy borders that reach past it.
            if let Some(rd) = get(&obj, "RD")?
                && let rd @ [_, _, _, _] = &numbers(&rd)?[..]
            {
                attrs.push(("fringe", join(rd, ",")));
            }
            if let Some(c) = get(&obj, "C")?
                && let Some(c) = hex(&numbers(&c)?)
            {
                attrs.push(("color", c));
            }
            for (key, attr) in [
                ("T", "title"),
                ("NM", "name"),
                ("M", "date"),
                ("Subj", "subject"),
            ] {
                if let Some(v) = string(&obj, key)? {
                    attrs.push((attr, v));
                }
            }
            if let Some(ca) = get(&obj, "CA")?
                && ca.is_number()?
            {
                attrs.push(("opacity", num(ca.as_float()?)));
            }
            if let Some(f) = get(&obj, "F")?
                && f.is_int()?
            {
                let bits = f.as_int()?;
                let set: Vec<&str> = (0..FLAGS.len())
                    .filter(|i| bits & (1 << i) != 0)
                    .map(|i| FLAGS[i])
                    .collect();
                if !set.is_empty() {
                    attrs.push(("flags", set.join(",")));
                }
            }
            if let Some(icon) = name(&obj, "Name")? {
                attrs.push(("icon", icon));
            }
            if let Some(irt) = get(&obj, "IRT")?
                && irt.is_indirect()?
                && let Some(parent) = string(&irt, "NM")?
            {
                attrs.push(("inreplyto", parent));
                if name(&obj, "RT")?.as_deref() == Some("Group") {
                    attrs.push(("replyType", "group".into()));
                }
            }
            for (key, attr) in [
                ("State", "state"),
                ("StateModel", "statemodel"),
                ("IT", "intent"),
            ] {
                if let Some(v) = name(&obj, key)? {
                    attrs.push((attr, v));
                }
            }
            if let Some(cl) = get(&obj, "CL")? {
                attrs.push(("callout", join(&numbers(&cl)?, ",")));
            }
            if subtype == "FreeText"
                && let Some(head) = name(&obj, "LE")?
            {
                attrs.push(("head", head));
            }
            if subtype == "Line"
                && let Some(le) = get(&obj, "LE")?
                && le.is_array()?
            {
                for (i, attr) in [(0, "head"), (1, "tail")] {
                    if let Some(n) = le.get_array(i)?
                        && n.is_name()?
                    {
                        attrs.push((attr, String::from_utf8_lossy(&n.as_name()?).into_owned()));
                    }
                }
            }
            if let Some(w) = border_width(&obj)? {
                attrs.push(("width", num(w)));
            }
            if let Some(be) = get(&obj, "BE")?
                && name(&be, "S")?.as_deref() == Some("C")
            {
                attrs.push(("style", "cloudy".into()));
                if let Some(i) = get(&be, "I")?
                    && i.is_number()?
                {
                    attrs.push(("intensity", num(i.as_float()?)));
                }
            } else if let Some(bs) = get(&obj, "BS")?
                && name(&bs, "S")?.as_deref() == Some("D")
            {
                attrs.push(("style", "dash".into()));
                if let Some(d) = get(&bs, "D")? {
                    attrs.push(("dashes", join(&numbers(&d)?, ",")));
                }
            }
            if let Some(ic) = get(&obj, "IC")?
                && let Some(ic) = hex(&numbers(&ic)?)
            {
                attrs.push(("interior-color", ic));
            }
            if let Some(q) = get(&obj, "QuadPoints")? {
                attrs.push(("coords", join(&numbers(&q)?, ",")));
            }
            if let Some(l) = get(&obj, "L")?
                && let [x1, y1, x2, y2] = numbers(&l)?[..]
            {
                attrs.push(("start", format!("{},{}", num(x1), num(y1))));
                attrs.push(("end", format!("{},{}", num(x2), num(y2))));
            }
            if let Some(contents) = string(&obj, "Contents")?
                && !contents.is_empty()
            {
                children += &format!("<contents>{}</contents>", escape(&contents));
            }
            if let Some(ink) = get(&obj, "InkList")? {
                children += "<inklist>";
                for stroke in ink.array_iter()? {
                    children += &format!("<gesture>{}</gesture>", points(&numbers(&stroke?)?));
                }
                children += "</inklist>";
            }
            if let Some(v) = get(&obj, "Vertices")? {
                children += &format!("<vertices>{}</vertices>", points(&numbers(&v)?));
            }
            if let Some(da) = string(&obj, "DA")? {
                children += &format!("<defaultappearance>{}</defaultappearance>", escape(&da));
            }
            if subtype == "Stamp"
                && let Some(ap) = get(&obj, "AP")?
            {
                let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
                write_object(&mut xml, Some(b"AP"), &ap, &mut Vec::new())?;
                children += &format!("<appearance>{}</appearance>", BASE64_STANDARD.encode(xml));
            }
            if subtype == "FileAttachment"
                && let Some(spec) = get(&obj, "FS")?
                && let Some(ef) = get(&spec, "EF")?
                && let Some(file) = get(&ef, "F")?
            {
                if let Some(name) = string(&spec, "UF")?.or(string(&spec, "F")?) {
                    attrs.push(("file", name));
                }
                if let Some(mime) = name(&file, "Subtype")? {
                    attrs.push(("mimetype", mime));
                }
                let data = file.read_stream()?;
                let hex: String = data.iter().map(|b| format!("{b:02X}")).collect();
                children += &format!(
                    "<data MODE=\"raw\" encoding=\"hex\" length=\"{}\">{hex}</data>",
                    data.len()
                );
            }
            out += &format!("<{tag}");
            for (k, v) in attrs {
                out += &format!(" {k}=\"{}\"", escape(&v));
            }
            if children.is_empty() {
                out += "/>\n";
            } else {
                out += &format!(">{children}</{tag}>\n");
            }
            count += 1;
        }
    }
    out += &format!("</annots>\n<f href=\"{}\"/>\n</xfdf>\n", escape(file));
    Ok((out, count))
}

/// Writes `obj` as Acrobat writes objects inside an XFDF <appearance>: one element per object
/// (DICT, STREAM, ARRAY, NAME, STRING, INT, FIXED, BOOL, NULL), named by KEY inside a dictionary,
/// with a stream's bytes, still encoded by its filters, in a DATA element. References are written
/// out in place; one that leads back to an object it is inside of becomes NULL.
fn write_object(
    out: &mut String,
    key: Option<&[u8]>,
    obj: &PdfObject,
    path: &mut Vec<i32>,
) -> Result<(), Error> {
    let key = key.map_or(String::new(), |k| {
        format!(" KEY=\"{}\"", escape(&String::from_utf8_lossy(k)))
    });
    let num = if obj.is_indirect()? {
        Some(obj.as_indirect()?)
    } else {
        None
    };
    let resolved = obj.resolve()?;
    let Some(value) =
        resolved.filter(|_| !num.is_some_and(|n| path.contains(&n)) && path.len() < MAX_DEPTH)
    else {
        *out += &format!("<NULL{key}/>");
        return Ok(());
    };
    path.push(num.unwrap_or(0));
    let stream = obj.is_stream()?;
    if stream || value.is_dict()? {
        let tag = if stream { "STREAM" } else { "DICT" };
        *out += &format!("<{tag}{key}>");
        for entry in value.dict_iter()? {
            let (k, v) = entry?;
            write_object(out, Some(&k.as_name()?), &v, path)?;
        }
        if stream {
            let data: String = obj
                .read_raw_stream()?
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect();
            *out += &format!("<DATA MODE=\"FILTERED\" ENCODING=\"HEX\">{data}</DATA>");
        }
        *out += &format!("</{tag}>");
    } else if value.is_array()? {
        *out += &format!("<ARRAY{key}>");
        for item in value.array_iter()? {
            write_object(out, None, &item?, path)?;
        }
        *out += "</ARRAY>";
    } else if value.is_name()? {
        let name = String::from_utf8_lossy(&value.as_name()?).into_owned();
        *out += &format!("<NAME{key} VAL=\"{}\"/>", escape(&name));
    } else if value.is_string()? {
        let bytes = value.as_bytes()?;
        match std::str::from_utf8(&bytes) {
            Ok(text) => *out += &format!("<STRING{key} VAL=\"{}\"/>", escape(text)),
            Err(_) => {
                let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
                *out += &format!("<STRING{key} VAL=\"{hex}\" ENCODING=\"HEX\"/>");
            }
        }
    } else if value.is_int()? {
        *out += &format!("<INT{key} VAL=\"{}\"/>", value.as_int()?);
    } else if value.is_real()? {
        *out += &format!("<FIXED{key} VAL=\"{}\"/>", num_exact(value.as_float()?));
    } else if value.is_bool()? {
        *out += &format!("<BOOL{key} VAL=\"{}\"/>", value.as_bool()?);
    } else {
        *out += &format!("<NULL{key}/>");
    }
    path.pop();
    Ok(())
}

/// A real number in full, since appearance matrices and boxes need every digit.
fn num_exact(n: f32) -> String {
    let s = n.to_string();
    if s.contains('.') { s } else { format!("{s}.0") }
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .map(|b| (b as char).to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()?;
    // An odd last digit stands for its high half, as in a PDF hex string.
    Some(
        digits
            .chunks(2)
            .map(|pair| pair[0] << 4 | pair.get(1).copied().unwrap_or(0))
            .collect(),
    )
}

/// Reads an object [`write_object`] wrote. Unknown elements and bad values give None.
fn read_object(
    pdf: &mut PdfDocument,
    node: roxmltree::Node,
    depth: usize,
) -> Result<Option<PdfObject>, Error> {
    if depth > MAX_DEPTH {
        return Ok(None);
    }
    let val = node.attribute("VAL");
    let elements = || node.children().filter(roxmltree::Node::is_element);
    Ok(Some(match node.tag_name().name() {
        "DICT" | "STREAM" => {
            let mut dict = pdf.new_dict()?;
            for child in elements() {
                if let Some(key) = child.attribute("KEY")
                    && let Some(value) = read_object(pdf, child, depth + 1)?
                {
                    dict.dict_put(key, value)?;
                }
            }
            if node.tag_name().name() == "DICT" {
                dict
            } else {
                let Some(data) = elements().find(|c| c.has_tag_name("DATA")) else {
                    return Ok(None);
                };
                let text = data.text().unwrap_or_default();
                let bytes = match data.attribute("ENCODING") {
                    Some("HEX") => match unhex(text) {
                        Some(bytes) => bytes,
                        None => return Ok(None),
                    },
                    _ => text.as_bytes().to_vec(),
                };
                // Filtered bytes keep the dictionary's filters; raw ones lose them.
                let filtered = data.attribute("MODE") == Some("FILTERED");
                pdf.add_stream(&Buffer::from_bytes(&bytes)?, Some(&dict), filtered)?
            }
        }
        "ARRAY" => {
            let mut array = pdf.new_array()?;
            for child in elements() {
                if let Some(value) = read_object(pdf, child, depth + 1)? {
                    array.array_push(value)?;
                }
            }
            array
        }
        "NAME" => PdfObject::new_name(val.unwrap_or_default())?,
        "STRING" => match (val, node.attribute("ENCODING")) {
            (Some(hex), Some("HEX")) if unhex(hex).is_some() => {
                pdf.new_object_from_str(&format!("<{hex}>"))?
            }
            (Some(text), _) => PdfObject::new_string(text)?,
            (None, _) => return Ok(None),
        },
        "INT" => match val.and_then(|v| v.parse().ok()) {
            Some(i) => PdfObject::new_int(i)?,
            None => return Ok(None),
        },
        "FIXED" => match val.and_then(|v| v.parse().ok()) {
            Some(f) => PdfObject::new_real(f)?,
            None => return Ok(None),
        },
        "BOOL" => PdfObject::new_bool(val == Some("true")),
        "NULL" => PdfObject::new_null(),
        _ => return Ok(None),
    }))
}

/// The file in a <fileattachment>'s <data>: its name and bytes, inflated when the XFDF
/// carries them compressed, as Acrobat writes them.
fn attached_file(
    pdf: &mut PdfDocument,
    node: roxmltree::Node,
) -> Result<Option<(String, Vec<u8>)>, Error> {
    let Some(data) = node.children().find(|n| n.has_tag_name("data")) else {
        return Ok(None);
    };
    let Some(mut bytes) = unhex(data.text().unwrap_or_default()) else {
        return Ok(None);
    };
    if let Some(filter) = data.attribute("filter") {
        // MuPDF decodes the stream through its filter; the stream is only a means to that.
        let dict = pdf.new_object_from_str(&format!("<</Filter /{}>>", pdf_name(filter)))?;
        let stream = pdf.add_stream(&Buffer::from_bytes(&bytes)?, Some(&dict), true)?;
        bytes = stream.read_stream()?;
        pdf.delete_object(stream.as_indirect()?)?;
    }
    let name = node
        .attribute("file")
        .and_then(|f| f.rsplit(['/', '\\']).next())
        .filter(|f| !f.is_empty())
        .unwrap_or("Attachment");
    Ok(Some((name.to_owned(), bytes)))
}

/// The appearance dictionary in an XFDF <appearance>, if it holds a usable one.
fn read_appearance(pdf: &mut PdfDocument, text: &str) -> Result<Option<PdfObject>, Error> {
    let packed: String = text.split_whitespace().collect();
    let Some(xml) = BASE64_STANDARD
        .decode(packed)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
    else {
        return Ok(None);
    };
    let Ok(tree) = roxmltree::Document::parse(&xml) else {
        return Ok(None);
    };
    let root = tree.root_element();
    if !root.has_tag_name("DICT") {
        return Ok(None);
    }
    let ap = read_object(pdf, root, 0)?;
    Ok(match ap {
        Some(ap) if ap.get_dict("N")?.is_some() => Some(ap),
        _ => None,
    })
}

/// A name for PDF syntax: letters and digits pass, anything else becomes #xx.
fn pdf_name(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("#{b:02X}")
            }
        })
        .collect()
}

fn parse_numbers(text: &str) -> Option<Vec<f32>> {
    text.split([',', ';', ' '])
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().parse().ok())
        .collect()
}

fn parse_color(text: &str) -> Option<[f32; 3]> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some([
        ((v >> 16) & 0xff) as f32 / 255.0,
        ((v >> 8) & 0xff) as f32 / 255.0,
        (v & 0xff) as f32 / 255.0,
    ])
}

fn array(pdf: &PdfDocument, values: &[f32]) -> Result<PdfObject, Error> {
    Ok(pdf.new_object_from_str(&format!("[{}]", join(values, " ")))?)
}

/// Adds the comments in `xml` as one undoable step. Comments whose name the document already
/// has are skipped, so importing the same file twice adds nothing. Returns how many were added.
pub fn import(doc: &Document, xml: &str) -> Result<usize, Error> {
    let tree = roxmltree::Document::parse(xml).map_err(|_| Error::Invalid("not an XFDF file"))?;
    let nodes: Vec<roxmltree::Node> = tree
        .descendants()
        .filter(|n| n.has_tag_name("annots"))
        .flat_map(|n| n.children().filter(roxmltree::Node::is_element))
        .collect();
    operation(doc, "Import comments", || {
        let mut pdf = PdfDocument::try_from(doc.clone()).map_err(|_| Error::NotPdf)?;
        let page_count = doc.page_count()?;
        let mut pages: HashMap<i32, PdfPage> = HashMap::new();
        // Every comment by name, so replies can point at their parent.
        let mut named: HashMap<String, PdfObject> = HashMap::new();
        for i in 0..page_count {
            let page = PdfPage::try_from(doc.load_page(i)?).map_err(|_| Error::NotPdf)?;
            for annot in page.annotations() {
                let obj = annot.object();
                if let Some(n) = string(&obj, "NM")? {
                    named.insert(n, obj);
                }
            }
            pages.insert(i, page);
        }
        let mut links = Vec::new();
        let mut added = 0;
        for node in nodes {
            let Some(&(_, _, subtype)) = KINDS.iter().find(|k| k.0 == node.tag_name().name())
            else {
                continue;
            };
            let Some(page_no) = node
                .attribute("page")
                .and_then(|p| p.parse::<i32>().ok())
                .filter(|p| (0..page_count).contains(p))
            else {
                continue;
            };
            let Some(rect) = node.attribute("rect").and_then(parse_numbers) else {
                continue;
            };
            let [x0, y0, x1, y1] = rect[..] else { continue };
            if node
                .attribute("name")
                .is_some_and(|n| named.contains_key(n))
            {
                continue;
            }
            // A file attachment without its file has nothing to open.
            let attached = if subtype == PdfAnnotationType::FileAttachment {
                let Some(file) = attached_file(&mut pdf, node)? else {
                    continue;
                };
                Some(file)
            } else {
                None
            };
            let page = pages.get_mut(&page_no).expect("every page is loaded");
            let mut annot = page.create_annotation(subtype)?;
            let mut obj = annot.object();
            if let Some((file, data)) = &attached {
                let options = EmbeddedFileOptions {
                    mime_type: node.attribute("mimetype"),
                    ..EmbeddedFileOptions::new(file)
                };
                obj.dict_put("FS", pdf.new_embedded_file(data, options)?)?;
            }
            if let Some(n) = node.attribute("name") {
                named.insert(n.to_owned(), annot.object());
            }
            if let Some(parent) = node.attribute("inreplyto") {
                let group = node.attribute("replyType") == Some("group");
                links.push((annot.object(), parent.to_owned(), group));
            }
            for (attr, key) in [
                ("state", "State"),
                ("statemodel", "StateModel"),
                ("intent", "IT"),
            ] {
                if let Some(v) = node.attribute(attr) {
                    obj.dict_put(key, PdfObject::new_name(v)?)?;
                }
            }
            if let Some(cl) = node.attribute("callout").and_then(parse_numbers) {
                obj.dict_put("CL", array(&pdf, &cl)?)?;
            }
            if subtype == PdfAnnotationType::FreeText
                && let Some(head) = node.attribute("head")
            {
                obj.dict_put("LE", PdfObject::new_name(head)?)?;
            }
            if subtype == PdfAnnotationType::Line
                && (node.has_attribute("head") || node.has_attribute("tail"))
            {
                let end = |attr| pdf_name(node.attribute(attr).unwrap_or("None"));
                obj.dict_put(
                    "LE",
                    pdf.new_object_from_str(&format!("[/{} /{}]", end("head"), end("tail")))?,
                )?;
            }
            obj.dict_put("Rect", array(&pdf, &[x0, y0, x1, y1])?)?;
            if let Some(rd) = node.attribute("fringe").and_then(parse_numbers)
                && rd.len() == 4
            {
                obj.dict_put("RD", array(&pdf, &rd)?)?;
            }
            for (attr, key) in [
                ("title", "T"),
                ("name", "NM"),
                ("date", "M"),
                ("subject", "Subj"),
            ] {
                if let Some(v) = node.attribute(attr) {
                    obj.dict_put(key, PdfObject::new_string(v)?)?;
                }
            }
            for (attr, key) in [("color", "C"), ("interior-color", "IC")] {
                if let Some(c) = node.attribute(attr).and_then(parse_color) {
                    obj.dict_put(key, array(&pdf, &c)?)?;
                }
            }
            if let Some(o) = node
                .attribute("opacity")
                .and_then(|o| o.parse::<f32>().ok())
            {
                obj.dict_put("CA", PdfObject::new_real(o)?)?;
            }
            if let Some(f) = node.attribute("flags") {
                let bits = f
                    .split(',')
                    .filter_map(|n| FLAGS.iter().position(|&f| f == n.trim()))
                    .fold(0, |bits, i| bits | (1 << i));
                obj.dict_put("F", PdfObject::new_int(bits)?)?;
            }
            if let Some(icon) = node.attribute("icon") {
                obj.dict_put("Name", PdfObject::new_name(icon)?)?;
            }
            let width = node.attribute("width").and_then(|w| w.parse::<f32>().ok());
            let style = node.attribute("style");
            let mut bs = String::new();
            if let Some(w) = width {
                bs += &format!("/W {}", num(w));
            }
            if style == Some("dash") {
                let dashes = node
                    .attribute("dashes")
                    .and_then(parse_numbers)
                    .unwrap_or_else(|| vec![3.0]);
                bs += &format!(" /S /D /D [{}]", join(&dashes, " "));
            }
            if !bs.is_empty() {
                obj.dict_put("BS", pdf.new_object_from_str(&format!("<<{bs}>>"))?)?;
            }
            if style == Some("cloudy") {
                let intensity = node
                    .attribute("intensity")
                    .and_then(|i| i.parse::<f32>().ok())
                    .unwrap_or(1.0);
                obj.dict_put(
                    "BE",
                    pdf.new_object_from_str(&format!("<</S /C /I {}>>", num(intensity)))?,
                )?;
            }
            if let Some(q) = node.attribute("coords").and_then(parse_numbers) {
                obj.dict_put("QuadPoints", array(&pdf, &q)?)?;
            }
            if let (Some(s), Some(e)) = (
                node.attribute("start").and_then(parse_numbers),
                node.attribute("end").and_then(parse_numbers),
            ) && let ([x1, y1], [x2, y2]) = (&s[..], &e[..])
            {
                obj.dict_put("L", array(&pdf, &[*x1, *y1, *x2, *y2])?)?;
            }
            for child in node.children().filter(roxmltree::Node::is_element) {
                match child.tag_name().name() {
                    "contents" => {
                        let text: String = child
                            .descendants()
                            .filter(|n| n.is_text())
                            .filter_map(|n| n.text())
                            .collect();
                        obj.dict_put("Contents", PdfObject::new_string(&text)?)?;
                    }
                    "inklist" => {
                        let strokes: Vec<String> = child
                            .children()
                            .filter(|n| n.has_tag_name("gesture"))
                            .filter_map(|g| parse_numbers(g.text().unwrap_or_default()))
                            .map(|s| format!("[{}]", join(&s, " ")))
                            .collect();
                        obj.dict_put(
                            "InkList",
                            pdf.new_object_from_str(&format!("[{}]", strokes.join(" ")))?,
                        )?;
                    }
                    "vertices" => {
                        if let Some(v) = parse_numbers(child.text().unwrap_or_default()) {
                            obj.dict_put("Vertices", array(&pdf, &v)?)?;
                        }
                    }
                    "defaultappearance" => {
                        let da = child.text().unwrap_or_default();
                        obj.dict_put("DA", PdfObject::new_string(da)?)?;
                    }
                    // MuPDF keeps this look for a stamp with its own icon name, such as a
                    // signature, and draws the standard stamps itself.
                    "appearance" => {
                        if let Some(ap) =
                            read_appearance(&mut pdf, child.text().unwrap_or_default())?
                        {
                            obj.dict_put("AP", ap)?;
                        }
                    }
                    _ => {}
                }
            }
            // Writing the dictionary does not tell MuPDF the look changed; setting the flags
            // (to what they are) does, so the update below redraws it.
            let flags = annot.flags()?;
            annot.set_flags(flags)?;
            added += 1;
        }
        // Parents can come after their replies in the file; link once all exist.
        for (mut obj, parent, group) in links {
            if let Some(parent) = named.get(&parent) {
                obj.dict_put("IRT", parent.try_clone()?)?;
                if group {
                    obj.dict_put("RT", PdfObject::new_name("Group")?)?;
                }
            }
        }
        for page in pages.values_mut() {
            page.update()?;
        }
        Ok(added)
    })
}
