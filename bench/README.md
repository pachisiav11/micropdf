# Benchmarks

`run.ps1` launches the release build with `--bench-scroll`, which scrolls top to bottom half a
screen per timer tick, waits 3 s, writes timings and exits. `measure.ps1` samples the private
working set (Task Manager's "Memory" column) summed over every process of the app, from outside,
the same way Acrobat is measured. Results land in `bench/results/` (git-ignored).

```powershell
python scripts/make_bench_fixtures.py          # fixtures/external/text-300.pdf, scan-1000.pdf
$env:CARGO_TARGET_DIR = "target/bench"; cargo build -p micropdf --release
./bench/run.ps1                                # micropdf only
./bench/run.ps1 -Acrobat                       # also Acrobat (close Acrobat first)
```

The runs open windows and take focus, so run them when nobody is using the machine.

## M0 renderer choice (2026-10-07)

Release build, Slint 1.18, 1500×960 physical window, whole-page renders (no tiles yet). Peak
private working set in MB; ticks/s counts scroll steps the event loop got through.

| renderer | no document | hello.pdf | text-300.pdf | scan-1000.pdf | ticks/s (text-300) |
|---|---|---|---|---|---|
| software | **8.6** | 16.3 | **52.8** | 276.6 | 561 |
| FemtoVG (OpenGL) | 23.3 | 96.0 | 242.0 | 413.4 | 76 |
| Skia | — | — | — | — | — |

Decision: **software renderer**. It uses 3–5× less memory and kept up with scrolling better.
Skia was not measured: its bundled libjpeg-turbo clashes with MuPDF's libjpeg at link time.

The scanned-document peak is over the 120 MB target. Known causes, addressed in M1: MuPDF's
resource store is fixed at 256 MB by `mupdf-sys`, and pages are rendered whole instead of as
viewport tiles.

The Acrobat comparison is still to be recorded.
