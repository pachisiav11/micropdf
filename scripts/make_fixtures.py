"""Writes the small, committed test PDFs into fixtures/.

outline-links.pdf  3 pages; outline with 3 entries; page 1 links to page 3 and to a URL.
form.pdf           AcroForm with a text field, a checkbox and a combo box.
truncated.pdf      hello.pdf cut before its xref table (MuPDF must repair it).
not-a-pdf.pdf      plain text with a .pdf name (must fail cleanly).
attachment.pdf     one page; embeds notes.txt in the EmbeddedFiles name tree.
layers.pdf         one page; two optional content groups, "Grid" on and "Notes" off.
cjk.pdf            one page; Chinese, Japanese and Korean lines in fonts that are not embedded,
                   so they render only through installed system fonts.
calc.pdf           AcroForm with JavaScript: total = a + b (calculate, two decimals format), and a
                   rejects values over 100 (validate).
xfa-static.pdf     AcroForm text field plus an XFA packet (a static XFA form).
xfa-dynamic.pdf    dynamic XFA: NeedsRendering, no AcroForm fields, a placeholder page.
redact.pdf         2 pages of uncompressed text with an email address, a phone number, a secret
                   word and invisible text; document information, an XMP packet, an opening
                   JavaScript action and an attached file, for redaction and Sanitize.

encrypted.pdf is written by MuPDF itself: cargo run -p mp-engine --example make_encrypted
"""
from pathlib import Path

FIXTURES = Path(__file__).resolve().parent.parent / "fixtures"


def serialize(objects: list[bytes], trailer: bytes = b"") -> bytes:
    """objects[0] is object 1 and must be the catalog."""
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for i, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % i + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    out += b"".join(b"%010d 00000 n \n" % off for off in offsets)
    out += b"trailer\n<< /Size %d /Root 1 0 R %s>>\nstartxref\n%d\n%%%%EOF\n" % (len(objects) + 1, trailer, xref)
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
    # 1 catalog, 2 pages, 3 page, 4 contents, 5 font, 6 text field, 7 checkbox, 8-9 checkbox appearances,
    # 10 combo box
    on = stream(b"q 0 0 0 rg BT /ZaDb 12 Tf 2 3 Td (4) Tj ET Q", b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] ")
    off = stream(b"", b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] ")
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 7 0 R 10 0 R] /NeedAppearances true "
        b"/DA (/Helv 12 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
        b"/Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R 7 0 R 10 0 R] >>",
        stream(b"BT /F1 14 Tf 72 700 Td (Name:) Tj 0 -40 Td (I agree:) Tj 0 -40 Td (Colour:) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V () /Rect [150 690 400 712] "
        b"/F 4 /P 3 0 R /DA (/Helv 12 Tf 0 g) /MK << /BC [0.5 0.5 0.5] >> >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Off /AS /Off /Rect [150 652 166 668] "
        b"/F 4 /P 3 0 R /MK << /BC [0.5 0.5 0.5] >> /AP << /N << /Yes 8 0 R /Off 9 0 R >> >> >>",
        on,
        off,
        b"<< /Type /Annot /Subtype /Widget /FT /Ch /Ff 131072 /T (colour) /V (Green) "
        b"/Opt [(Red) (Green) (Blue)] /Rect [150 612 300 630] /F 4 /P 3 0 R /DA (/Helv 12 Tf 0 g) "
        b"/MK << /BC [0.5 0.5 0.5] >> >>",
    ]
    return serialize(objs)


ATTACHMENT_TEXT = b"Embedded by make_fixtures.py.\n"


def attachment() -> bytes:
    # 1 catalog, 2 pages, 3 page, 4 contents, 5 font, 6 filespec, 7 embedded file
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(notes.txt) 6 0 R] >> >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
        b"/Resources << /Font << /F1 5 0 R >> >> >>",
        stream(b"BT /F1 14 Tf 72 700 Td (This document carries an attachment.) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /Filespec /F (notes.txt) /UF (notes.txt) /EF << /F 7 0 R >> >>",
        stream(ATTACHMENT_TEXT, b"/Type /EmbeddedFile /Params << /Size %d >> " % len(ATTACHMENT_TEXT)),
    ]
    return serialize(objs)


def layers() -> bytes:
    # 1 catalog, 2 pages, 3 page, 4 contents, 5 font, 6-7 optional content groups
    content = (b"BT /F1 14 Tf 72 700 Td (Always visible) Tj ET\n"
               b"/OC /oc1 BDC 0.6 g 72 600 200 40 re f EMC\n"
               b"/OC /oc2 BDC BT /F1 14 Tf 72 560 Td (Hidden notes layer) Tj ET EMC")
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R 7 0 R] "
        b"/D << /Order [6 0 R 7 0 R] /ON [6 0 R] /OFF [7 0 R] >> >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
        b"/Resources << /Font << /F1 5 0 R >> /Properties << /oc1 6 0 R /oc2 7 0 R >> >> >>",
        stream(content),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /OCG /Name (Grid) >>",
        b"<< /Type /OCG /Name (Notes) >>",
    ]
    return serialize(objs)


def cjk() -> bytes:
    # 1 catalog, 2 pages, 3 page, 4 contents, then per script: Type0 font, CIDFont, descriptor
    scripts = [
        (b"STSong-Light", b"UniGB-UCS2-H", b"GB1", "中文文本"),
        (b"KozMinPro-Regular", b"UniJIS-UCS2-H", b"Japan1", "日本語"),
        (b"HYSMyeongJo-Medium", b"UniKS-UCS2-H", b"Korea1", "한국어"),
    ]
    content = b""
    fonts = b""
    objs: list[bytes] = []
    for n, (name, cmap, ordering, text) in enumerate(scripts):
        first = 5 + 3 * n
        fonts += b"/F%d %d 0 R " % (n + 1, first)
        content += b"BT /F%d 36 Tf 40 %d Td <%s> Tj ET\n" % (
            n + 1, 300 - 90 * n, text.encode("utf-16-be").hex().upper().encode())
        objs += [
            b"<< /Type /Font /Subtype /Type0 /BaseFont /%s /Encoding /%s /DescendantFonts [%d 0 R] >>"
            % (name, cmap, first + 1),
            b"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /%s /CIDSystemInfo << /Registry (Adobe) "
            b"/Ordering (%s) /Supplement 2 >> /FontDescriptor %d 0 R /DW 1000 >>" % (name, ordering, first + 2),
            b"<< /Type /FontDescriptor /FontName /%s /Flags 6 /FontBBox [-25 -254 1000 880] "
            b"/ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 880 /StemV 93 >>" % name,
        ]
    return serialize([
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 360] /Contents 4 0 R "
        b"/Resources << /Font << %s>> >> >>" % fonts,
        stream(content.rstrip()),
        *objs,
    ])


def js(code: bytes) -> bytes:
    return b"<< /S /JavaScript /JS (" + code + b") >>"


def text_field(name: bytes, rect: bytes, extra: bytes = b"") -> bytes:
    return (b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (" + name + b") /V () /Rect [" + rect +
            b"] /F 4 /P 3 0 R /DA (/Helv 12 Tf 0 g) /MK << /BC [0.5 0.5 0.5] >> " + extra + b">>")


def calc() -> bytes:
    # 1 catalog, 2 pages, 3 page, 4 contents, 5 font, 6-8 fields a, b, total
    total_actions = (b"/AA << /C " + js(b'AFSimple_Calculate("SUM", new Array ("a", "b"));') +
                     b" /F " + js(b'AFNumber_Format(2, 0, 0, 0, "", true);') + b" >> ")
    a_actions = b"/AA << /V " + js(b"if (event.value > 100) event.rc = false;") + b" >> "
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 7 0 R 8 0 R] /CO [8 0 R] "
        b"/DA (/Helv 12 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
        b"/Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R 7 0 R 8 0 R] >>",
        stream(b"BT /F1 14 Tf 72 700 Td (A:) Tj 0 -40 Td (B:) Tj 0 -40 Td (Total:) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        text_field(b"a", b"150 690 300 712", a_actions),
        text_field(b"b", b"150 650 300 672"),
        text_field(b"total", b"150 610 300 632", total_actions),
    ]
    return serialize(objs)


XFA_PACKET = (b'<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">'
              b'<template xmlns="http://www.xfa.org/schema/xfa-template/2.8/"/></xdp:xdp>')


def xfa_static() -> bytes:
    # 1 catalog, 2 pages, 3 page, 4 contents, 5 font, 6 text field, 7 XFA packet
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] /XFA 7 0 R "
        b"/DA (/Helv 12 Tf 0 g) /DR << /Font << /Helv 5 0 R >> >> >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
        b"/Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>",
        stream(b"BT /F1 14 Tf 72 700 Td (Name:) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        text_field(b"name", b"150 690 400 712"),
        stream(XFA_PACKET),
    ]
    return serialize(objs)


def xfa_dynamic() -> bytes:
    # 1 catalog, 2 pages, 3 page, 4 contents, 5 font, 6 XFA packet
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R /NeedsRendering true /AcroForm << /Fields [] /XFA 6 0 R >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
        b"/Resources << /Font << /F1 5 0 R >> >> >>",
        stream(b"BT /F1 14 Tf 72 700 Td (Please wait... this form needs Adobe Reader.) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        stream(XFA_PACKET),
    ]
    return serialize(objs)


def redact() -> bytes:
    # 1 catalog, 2 pages, 3-4 pages, 5-6 contents, 7 font, 8 info, 9 XMP, 10 filespec, 11 file
    page1 = (b"BT /F1 14 Tf 72 700 Td (Contact jane.doe@example.com or call \\(555\\) 123-4567.) Tj ET\n"
             b"BT /F1 14 Tf 72 670 Td (Project SECRET-PLAN starts on Monday.) Tj ET\n"
             b"BT 3 Tr /F1 14 Tf 72 640 Td (HIDDEN-OCR-LAYER) Tj ET")
    secret = b"attachment-secret"
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R /Metadata 9 0 R /OpenAction << /S /JavaScript /JS (app.alert\\('hi'\\)) >> "
        b"/Names << /EmbeddedFiles << /Names [(secret.txt) 10 0 R] >> >> >>",
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> >> >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >>",
        stream(page1),
        stream(b"BT /F1 14 Tf 72 700 Td (Page two keeps its text.) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Title (Redaction test) /Author (Jane Doe) >>",
        stream(b"<x:xmpmeta xmlns:x='adobe:ns:meta/'>xmp-secret</x:xmpmeta>", b"/Type /Metadata /Subtype /XML "),
        b"<< /Type /Filespec /F (secret.txt) /UF (secret.txt) /EF << /F 11 0 R >> >>",
        stream(secret, b"/Type /EmbeddedFile /Params << /Size %d >> " % len(secret)),
    ]
    return serialize(objs, b"/Info 8 0 R ")


def main() -> None:
    (FIXTURES / "outline-links.pdf").write_bytes(outline_links())
    (FIXTURES / "form.pdf").write_bytes(form())
    hello = (FIXTURES / "hello.pdf").read_bytes()
    (FIXTURES / "truncated.pdf").write_bytes(hello[: hello.index(b"xref")])
    (FIXTURES / "not-a-pdf.pdf").write_bytes(b"This is a text file, not a PDF.\n")
    (FIXTURES / "attachment.pdf").write_bytes(attachment())
    (FIXTURES / "layers.pdf").write_bytes(layers())
    (FIXTURES / "cjk.pdf").write_bytes(cjk())
    (FIXTURES / "calc.pdf").write_bytes(calc())
    (FIXTURES / "xfa-static.pdf").write_bytes(xfa_static())
    (FIXTURES / "xfa-dynamic.pdf").write_bytes(xfa_dynamic())
    (FIXTURES / "redact.pdf").write_bytes(redact())
    print("wrote outline-links.pdf, form.pdf, truncated.pdf, not-a-pdf.pdf, attachment.pdf, layers.pdf, "
          "cjk.pdf, calc.pdf, xfa-static.pdf, xfa-dynamic.pdf, redact.pdf")


if __name__ == "__main__":
    main()
