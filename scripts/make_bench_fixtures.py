"""Writes the large benchmark PDFs into fixtures/external/ (git-ignored).

text-300.pdf  300 Letter pages of Helvetica text, 52 lines each.
scan-1000.pdf 1000 Letter pages, each one full-page 150 dpi greyscale image (like a scan).
"""
import random
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "fixtures" / "external"
LETTER = (612, 792)
WORDS = ("lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor "
         "incididunt ut labore et dolore magna aliqua enim ad minim veniam quis nostrud").split()


def write_pdf(path: Path, pages: list[tuple[bytes, dict[str, bytes]]]) -> None:
    """pages: (content stream, {resource name: object body}) per page. Objects 1-2 are fixed."""
    objects: list[bytes] = [b"", b""]  # catalog, pages: filled in last
    font = len(objects) + 1
    objects.append(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    kids = []
    for content, images in pages:
        xobjects = []
        for name, body in images.items():
            objects.append(body)
            xobjects.append(b"/%s %d 0 R" % (name.encode(), len(objects)))
        objects.append(b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream")
        contents = len(objects)
        objects.append(
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %d %d] /Contents %d 0 R "
            b"/Resources << /Font << /F1 %d 0 R >> /XObject << %s >> >> >>"
            % (LETTER[0], LETTER[1], contents, font, b" ".join(xobjects))
        )
        kids.append(len(objects))
    objects[0] = b"<< /Type /Catalog /Pages 2 0 R >>"
    objects[1] = b"<< /Type /Pages /Kids [%s] /Count %d >>" % (
        b" ".join(b"%d 0 R" % k for k in kids), len(kids))

    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for i, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % i + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    out += b"".join(b"%010d 00000 n \n" % off for off in offsets)
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objects) + 1, xref)
    path.write_bytes(out)
    print(f"wrote {path} ({len(out) / 1e6:.1f} MB, {len(pages)} pages)")


def text_page(rng: random.Random, number: int) -> bytes:
    lines = [b"BT /F1 10 Tf 13 TL 56 740 Td"]
    lines.append(b"(Page %d) Tj T*" % number)
    for _ in range(52):
        line = " ".join(rng.choice(WORDS) for _ in range(rng.randint(10, 14)))
        lines.append(b"(%s) Tj T*" % line.encode())
    lines.append(b"ET")
    return b"\n".join(lines)


SCAN_W, SCAN_H = 1275, 1650


def noisy_rows(rng: random.Random, base: int, count: int) -> list[bytes]:
    rows = []
    for _ in range(count):
        cells = bytes(max(0, min(255, base + rng.randint(-12, 12))) for _ in range(0, SCAN_W, 15))
        rows.append(bytes(b for b in cells for _ in range(15))[:SCAN_W])
    return rows


def scan_image(rng: random.Random, paper: list[bytes], ink: list[bytes]) -> bytes:
    """150 dpi grey page: light paper noise with dark 'text line' bands, shuffled per page."""
    rows = []
    for y in range(SCAN_H):
        in_line = 150 < y < 1500 and (y // 12) % 2 == 0 and rng.random() > 0.08
        rows.append(rng.choice(ink if in_line else paper))
    w, h = SCAN_W, SCAN_H
    data = zlib.compress(b"".join(rows), 1)
    return (b"<< /Type /XObject /Subtype /Image /Width %d /Height %d /ColorSpace /DeviceGray "
            b"/BitsPerComponent 8 /Filter /FlateDecode /Length %d >>\nstream\n" % (w, h, len(data))
            + data + b"\nendstream")


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    rng = random.Random(42)
    write_pdf(OUT / "text-300.pdf", [(text_page(rng, n), {}) for n in range(1, 301)])
    # One distinct image per page so nothing is shared between pages, as in a real scan.
    paper, ink = noisy_rows(rng, 235, 48), noisy_rows(rng, 40, 48)
    write_pdf(OUT / "scan-1000.pdf",
              [(b"q 612 0 0 792 0 0 cm /Im0 Do Q", {"Im0": scan_image(rng, paper, ink)})
               for _ in range(1000)])


if __name__ == "__main__":
    main()
