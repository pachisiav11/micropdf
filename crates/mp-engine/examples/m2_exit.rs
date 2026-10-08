//! Writes the PDFs for the M2 exit check into the folder given as the first argument (default:
//! target/m2-exit): every comment kind and a filled form, saved in full, saved incrementally,
//! carried to a fresh copy through XFDF and FDF, flattened, and a form with JavaScript totals.
//! scripts/check_pdfs.py then parses them strictly and draws them with PDFium, Chrome's engine.

use std::path::{Path, PathBuf};

use mp_engine::{
    AnnotKind, Border, DocId, Engine, Error, FieldEdit, Mark, NewAnnot, Properties, Rect, Restyle,
    Style,
};

const RED: [f32; 3] = [0.85, 0.12, 0.1];
const BLUE: [f32; 3] = [0.1, 0.3, 0.8];
const YELLOW: [f32; 3] = [1.0, 0.85, 0.0];

fn style(color: [f32; 3]) -> Style {
    Style {
        color,
        author: "Ada".into(),
    }
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect { x0, y0, x1, y1 }
}

/// Fills form.pdf: a name, the checkbox and a colour.
fn fill_form(engine: &Engine, doc: DocId) -> Result<(), Error> {
    for field in engine.fields(doc, 0)? {
        let edit = match field.name.as_str() {
            "name" => FieldEdit::Value("Ada Lovelace".into()),
            "agree" => FieldEdit::Toggle,
            "colour" => FieldEdit::Value("Blue".into()),
            _ => continue,
        };
        engine.edit_field(doc, 0, field.id, edit)?;
    }
    Ok(())
}

/// Adds one comment of every kind to form.pdf's page (612 x 792 pt, y down), styled and with
/// properties set, plus a typed signature and drawn initials.
fn comment(engine: &Engine, doc: DocId) -> Result<(), Error> {
    let add = |new, color| engine.add_annotation(doc, 0, new, style(color));
    let restyle = |id, change| engine.restyle(doc, 0, id, change);
    let markup = |kind, r| NewAnnot::TextMarkup {
        kind,
        rects: vec![r],
    };

    add(
        markup(AnnotKind::Highlight, rect(70.0, 79.0, 116.0, 97.0)),
        YELLOW,
    )?;
    add(
        markup(AnnotKind::Underline, rect(70.0, 119.0, 122.0, 137.0)),
        BLUE,
    )?;
    add(
        markup(AnnotKind::StrikeOut, rect(70.0, 159.0, 119.0, 177.0)),
        RED,
    )?;
    add(
        markup(AnnotKind::Squiggly, rect(152.0, 82.0, 240.0, 100.0)),
        RED,
    )?;

    let note = add(
        NewAnnot::Note {
            x: 450.0,
            y: 110.0,
            text: "A sticky note".into(),
        },
        YELLOW,
    )?;
    engine.reply(doc, 0, note.id, "A reply".into(), "Grace".into())?;
    engine.set_state(doc, 0, note.id, "Accepted".into(), "Grace".into())?;

    let text = add(
        NewAnnot::FreeText {
            rect: rect(72.0, 215.0, 300.0, 262.0),
            text: "Text box: café, naïve, 14 pt".into(),
        },
        RED,
    )?;
    restyle(text.id, Restyle::FontSize(14.0))?;
    restyle(text.id, Restyle::Fill(Some([1.0, 1.0, 0.8])))?;

    let ink = add(
        NewAnnot::Ink {
            strokes: vec![vec![
                (340.0, 260.0),
                (380.0, 220.0),
                (420.0, 260.0),
                (460.0, 220.0),
                (500.0, 260.0),
                (540.0, 220.0),
            ]],
            width: 2.0,
        },
        BLUE,
    )?;
    restyle(ink.id, Restyle::Opacity(0.7))?;

    let square = add(
        NewAnnot::Shape {
            kind: AnnotKind::Square,
            rect: rect(72.0, 300.0, 260.0, 380.0),
            width: 2.0,
        },
        RED,
    )?;
    restyle(square.id, Restyle::Fill(Some([1.0, 0.92, 0.9])))?;
    restyle(square.id, Restyle::Border(Border::Cloudy))?;
    engine.set_properties(
        doc,
        0,
        square.id,
        Properties {
            author: "Ada".into(),
            subject: "Locked cloud".into(),
            locked: true,
            printed: true,
        },
    )?;

    let circle = add(
        NewAnnot::Shape {
            kind: AnnotKind::Circle,
            rect: rect(320.0, 300.0, 540.0, 380.0),
            width: 2.0,
        },
        BLUE,
    )?;
    restyle(circle.id, Restyle::Border(Border::Dashed))?;
    restyle(circle.id, Restyle::Width(3.0))?;

    let line = add(
        NewAnnot::Line {
            from: (72.0, 420.0),
            to: (300.0, 450.0),
            width: 2.0,
        },
        RED,
    )?;
    restyle(line.id, Restyle::LineEnds("Circle", "ClosedArrow"))?;
    restyle(line.id, Restyle::Border(Border::Dashed))?;

    add(
        NewAnnot::Callout {
            target: (380.0, 430.0),
            rect: rect(420.0, 450.0, 570.0, 500.0),
            text: "Callout".into(),
        },
        BLUE,
    )?;
    add(
        NewAnnot::Stamp {
            name: "Approved".into(),
            center: (170.0, 540.0),
            width: 150.0,
        },
        [0.1, 0.5, 0.2],
    )?;
    add(
        NewAnnot::File {
            at: (520.0, 520.0),
            name: "notes.txt".into(),
            data: b"M2 exit check\n".to_vec(),
        },
        BLUE,
    )?;

    let font = ["Segoe Script", "Ink Free", "Arial"]
        .into_iter()
        .find(|f| mp_engine::has_font(f))
        .unwrap_or("Arial");
    engine.place_mark(
        doc,
        0,
        Mark::Typed {
            text: "Ada Lovelace".into(),
            font: font.into(),
        },
        (180.0, 650.0),
        180.0,
        [0.05, 0.1, 0.45],
    )?;
    engine.place_mark(
        doc,
        0,
        Mark::Ink {
            strokes: vec![
                vec![(0.0, 40.0), (10.0, 0.0), (20.0, 40.0)],
                vec![(5.0, 22.0), (15.0, 22.0)],
                vec![
                    (30.0, 40.0),
                    (30.0, 0.0),
                    (45.0, 10.0),
                    (45.0, 30.0),
                    (30.0, 40.0),
                ],
            ],
            width: 3.0,
        },
        (450.0, 650.0),
        60.0,
        [0.05, 0.1, 0.45],
    )?;
    Ok(())
}

fn main() -> Result<(), Error> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/m2-exit"));
    std::fs::create_dir_all(&out)?;
    let fixture = |name: &str| root.join("fixtures").join(name);
    let path = |name: &str| out.join(name);
    let engine = Engine::start();

    // The same edits, saved in full and appended to a copy of the original.
    let doc = engine.open(fixture("form.pdf"))?.id;
    fill_form(&engine, doc)?;
    comment(&engine, doc)?;
    // A full save may tidy the document in memory, so the incremental one goes first.
    engine.save(doc, &path("incremental.pdf"), true)?;
    engine.save(doc, &path("full.pdf"), false)?;

    // Comments through XFDF and field values through FDF, into a fresh copy.
    let (xfdf, comments) = engine.export_comments(doc, "form.pdf".into())?;
    let fdf = engine.export_fdf(doc, "form.pdf".into())?;
    std::fs::write(path("comments.xfdf"), &xfdf)?;
    std::fs::write(path("fields.fdf"), &fdf)?;
    let fresh = engine.open(fixture("form.pdf"))?.id;
    let imported = engine.import_comments(fresh, xfdf)?;
    engine.import_fdf(fresh, fdf)?;
    engine.save(fresh, &path("imported.pdf"), false)?;
    println!("XFDF: exported {comments} comments, imported {imported}");

    engine.flatten(doc, true, true)?;
    engine.save(doc, &path("flattened.pdf"), false)?;

    // JavaScript runs as the fields change: total = a + b, two decimals.
    let calc = engine.open(fixture("calc.pdf"))?.id;
    for field in engine.fields(calc, 0)? {
        let value = match field.name.as_str() {
            "a" => "1.5",
            "b" => "2.25",
            _ => continue,
        };
        engine.edit_field(calc, 0, field.id, FieldEdit::Value(value.into()))?;
    }
    engine.save(calc, &path("calc.pdf"), false)?;

    for name in [
        "full.pdf",
        "incremental.pdf",
        "imported.pdf",
        "flattened.pdf",
        "calc.pdf",
    ] {
        report(&engine, &path(name))?;
    }
    Ok(())
}

/// Opens a written file again and prints what micropdf reads back from it.
fn report(engine: &Engine, file: &Path) -> Result<(), Error> {
    let doc = engine.open(file)?.id;
    let annots = engine.annotations(doc, 0)?;
    let fields: Vec<String> = engine
        .fields(doc, 0)?
        .into_iter()
        .map(|f| format!("{}={:?}", f.name, f.value))
        .collect();
    println!(
        "{}: {} comments, fields [{}]",
        file.display(),
        annots.len(),
        fields.join(", ")
    );
    engine.close(doc);
    Ok(())
}
