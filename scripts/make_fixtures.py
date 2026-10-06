"""Writes the small, committed test PDFs into fixtures/.

outline-links.pdf  3 pages; outline with 3 entries; page 1 links to page 3 and to a URL.
form.pdf           AcroForm with a text field and a checkbox.
truncated.pdf      hello.pdf cut before its xref table (MuPDF must repair it).
not-a-pdf.pdf      plain text with a .pdf name (must fail cleanly).

encrypted.pdf is written by MuPDF itself: cargo run -p mp-engine --example make_encrypted
"""
from pathlib import Path

FIXTURES = Path(__file__).resolve().parent.parent / "fixtures"


def serialize(objects: list[bytes]) -> bytes:
    """objects[0] is object 1 and must be the catalog."""
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for i, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % i + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    out += b"".join(b"%010d 00000 n \n" % off for off in offsets)
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objects) + 1, xref)
    return bytes(out)


def stream(content: bytes, extra: bytes = b"") -> bytes:
    return b"<< /Length %d %s>>\nstream\n" % (len(content), extra) + content + b"\nendstream"


def outline_links() -> bytes:
    # 1 catalog, 2 pages, 3 font, 4-6 pages, 7-9 contents, 10 outlines, 11-13 items, 14-15 links
    objs: list[bytes] = [
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 10 0 R /PageMode /UseOutlines >>",
        b"<< /Type /Pages /Kids [4 0 R 5 0 R 6 0 R] /Count 3 >>",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    for n in range(3):
        annots = b"/Annots [14 0 R 15 0 R]" if n == 0 else b""
        objs.append(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents %d 0 R "
                    b"/Resources << /Font << /F1 3 0 R >> >> %s >>" % (7 + n, annots))
    for n in range(3):
        text = b"BT /F1 28 Tf 72 700 Td (Chapter %d) Tj ET" % (n + 1)
        if n == 0:
            text += (b"\nBT /F1 14 Tf 72 640 Td (Go to chapter 3) Tj ET"
                     b"\nBT /F1 14 Tf 72 610 Td (Visit example.com) Tj ET")
        objs.append(stream(text))
    objs.append(b"<< /Type /Outlines /First 11 0 R /Last 13 0 R /Count 3 >>")
    for n in range(3):
        prev = b"/Prev %d 0 R" % (10 + n) if n > 0 else b""
        nxt = b"/Next %d 0 R" % (12 + n) if n < 2 else b""
        objs.append(b"<< /Title (Chapter %d) /Parent 10 0 R %s %s /Dest [%d 0 R /XYZ 0 792 0] >>"
                    % (n + 1, prev, nxt, 4 + n))
    objs.append(b"<< /Type /Annot /Subtype /Link /Rect [70 635 220 655] /Border [0 0 0] "
                b"/Dest [6 0 R /XYZ 0 792 0] >>")
    objs.append(b"<< /Type /Annot /Subtype /Link /Rect [70 605 220 625] /Border [0 0 0] "
                b"/A << /S /URI /URI (https://example.com/) >> >>")
    return serialize(objs)


def form() -> bytes:
    # 1 catalog, 2 pages, 3 page, 4 contents, 5 font, 6 text field, 7 checkbox, 8-9 checkbox appearances
    on = stream(b"q 0 0 0 rg BT /ZaDb 12 Tf 2 3 Td (4) Tj ET Q", b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] ")
    off = stream(b"", b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] ")
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 7 0 R] /NeedAppearances true "
        b"/DA (/Helv 12 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
        b"/Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R 7 0 R] >>",
        stream(b"BT /F1 14 Tf 72 700 Td (Name:) Tj 0 -40 Td (I agree:) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V () /Rect [150 690 400 712] "
        b"/F 4 /P 3 0 R /DA (/Helv 12 Tf 0 g) /MK << /BC [0.5 0.5 0.5] >> >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [150 652 166 668] "
        b"/F 4 /P 3 0 R /MK << /BC [0.5 0.5 0.5] >> /AP << /N << /Yes 8 0 R /Off 9 0 R >> >> >>",
        on,
        off,
    ]
    return serialize(objs)


def main() -> None:
    (FIXTURES / "outline-links.pdf").write_bytes(outline_links())
    (FIXTURES / "form.pdf").write_bytes(form())
    hello = (FIXTURES / "hello.pdf").read_bytes()
    (FIXTURES / "truncated.pdf").write_bytes(hello[: hello.index(b"xref")])
    (FIXTURES / "not-a-pdf.pdf").write_bytes(b"This is a text file, not a PDF.\n")
    print("wrote outline-links.pdf, form.pdf, truncated.pdf, not-a-pdf.pdf")


if __name__ == "__main__":
    main()
