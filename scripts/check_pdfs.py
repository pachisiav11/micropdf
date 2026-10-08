"""Checks PDFs the way other readers see them: pypdf parses each file strictly, and PDFium, the
engine in Chrome's PDF viewer, counts the comments and fields and draws every page.

    cargo run -p mp-engine --example m2_exit
    python scripts/check_pdfs.py target/m2-exit/*.pdf --png target/m2-exit/png

Exits 1 when a file does not parse cleanly, a visible comment or field has no appearance stream,
or PDFium sees a different number of annotations than pypdf.
"""
import argparse
import logging
import sys
from collections import Counter
from pathlib import Path

import pypdf
import pypdfium2 as pdfium
import pypdfium2.raw as raw

HIDDEN = 2

# pypdf warns when the newest cross-reference section does not start at object 0. An incremental
# update lists only the objects it changes, so it rarely does; pypdf still checks every object
# number against the file, and fails on a mismatch, so the warning alone says nothing is wrong.
BENIGN = ("Xref table not zero-indexed",)

# PDFium's FPDF_ANNOTATION_SUBTYPE values, for the report.
SUBTYPES = {
    1: "Text", 2: "Link", 3: "FreeText", 4: "Line", 5: "Square", 6: "Circle", 7: "Polygon",
    8: "PolyLine", 9: "Highlight", 10: "Underline", 11: "Squiggly", 12: "StrikeOut", 13: "Stamp",
    14: "Caret", 15: "Ink", 16: "Popup", 17: "FileAttachment", 18: "Sound", 19: "Movie",
    20: "Widget", 21: "Screen", 22: "PrinterMark", 23: "TrapNet", 24: "Watermark", 25: "3D",
    26: "RichMedia", 27: "XFAWidget", 28: "Redact",
}


class Warnings(logging.Handler):
    def __init__(self) -> None:
        super().__init__(logging.WARNING)
        self.messages: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.messages.append(record.getMessage())


def with_pypdf(path: Path) -> tuple[list[str], Counter, dict[str, str]]:
    """Problems, annotation subtypes per page summed, and field values."""
    problems: list[str] = []
    kinds: Counter = Counter()
    values: dict[str, str] = {}
    warnings = Warnings()
    log = logging.getLogger("pypdf")
    log.addHandler(warnings)
    try:
        reader = pypdf.PdfReader(path, strict=True)
        for number, page in enumerate(reader.pages, start=1):
            page.extract_text()
            for ref in page.get("/Annots") or []:
                annot = ref.get_object()
                kind = str(annot.get("/Subtype", "?")).lstrip("/")
                kinds[kind] += 1
                visible = not int(annot.get("/F", 0)) & HIDDEN
                if visible and kind not in ("Popup", "Link") and "/AP" not in annot:
                    problems.append(f"page {number}: a {kind} annotation has no appearance stream")
        for name, field in (reader.get_fields() or {}).items():
            if "/V" in field:
                values[name] = str(field["/V"])
    except Exception as e:  # noqa: BLE001 - any parse failure is the finding
        problems.append(f"pypdf: {type(e).__name__}: {e}")
    finally:
        log.removeHandler(warnings)
    problems += [
        f"pypdf warning: {m}" for m in warnings.messages if not m.startswith(BENIGN)
    ]
    return problems, kinds, values


def with_pdfium(path: Path, png: Path | None) -> tuple[list[str], Counter]:
    problems: list[str] = []
    kinds: Counter = Counter()
    try:
        pdf = pdfium.PdfDocument(str(path))
        pdf.init_forms()
        for number, page in enumerate(pdf, start=1):
            for i in range(raw.FPDFPage_GetAnnotCount(page.raw)):
                annot = raw.FPDFPage_GetAnnot(page.raw, i)
                kinds[SUBTYPES.get(raw.FPDFAnnot_GetSubtype(annot), "?")] += 1
                raw.FPDFPage_CloseAnnot(annot)
            if png is not None:
                image = page.render(scale=1.5, may_draw_forms=True).to_pil()
                image.save(png / f"{path.stem}-{number}.png")
    except Exception as e:  # noqa: BLE001
        problems.append(f"PDFium: {type(e).__name__}: {e}")
    return problems, kinds


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("files", nargs="+", type=Path)
    parser.add_argument("--png", type=Path, help="write each page as drawn by PDFium here")
    args = parser.parse_args()
    if args.png:
        args.png.mkdir(parents=True, exist_ok=True)
    failed = False
    for path in args.files:
        problems, kinds, values = with_pypdf(path)
        more, pdfium_kinds = with_pdfium(path, args.png)
        problems += more
        if pdfium_kinds != kinds:
            problems.append(f"PDFium sees {dict(pdfium_kinds)}, pypdf sees {dict(kinds)}")
        status = "FAIL" if problems else "ok"
        print(f"{status}  {path.name}: {sum(kinds.values())} annotations {dict(sorted(kinds.items()))}")
        if values:
            print("      fields " + ", ".join(f"{k}={v}" for k, v in values.items()))
        for p in problems:
            print(f"      {p}")
        failed |= bool(problems)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
