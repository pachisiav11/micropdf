//! Printing through GDI: the system print dialog, then each page rendered at the printer's
//! resolution (capped) and stretched onto the printable area. Runs on its own thread.

use std::sync::atomic::{AtomicBool, Ordering};

use mp_engine::Tile;
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteDC, GetDeviceCaps, HORZRES,
    SRCCOPY, StretchDIBits, VERTRES,
};
use windows_sys::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
use windows_sys::Win32::UI::Controls::Dialogs::{
    PD_NOSELECTION, PD_PAGENUMS, PD_RETURNDC, PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW, PrintDlgW,
};

use crate::viewer;

/// Pages are rendered at most this many dots per inch; enough for text, and keeps a letter
/// page under 30 MB of pixels.
const MAX_DPI: f32 = 300.0;

static PRINTING: AtomicBool = AtomicBool::new(false);

pub fn start() {
    let Some((engine, (doc, sizes), path)) =
        viewer::with(|app| Some((app.engine(), app.active_doc()?, app.active_path()?))).flatten()
    else {
        return;
    };
    if PRINTING.swap(true, Ordering::SeqCst) {
        return;
    }
    let title = viewer::file_name(&path);
    std::thread::spawn(move || {
        let result = print(&engine, doc, &sizes, &title);
        PRINTING.store(false, Ordering::SeqCst);
        let message = match result {
            Ok(Some(n)) => format!(
                "Sent {n} page{} to the printer",
                if n == 1 { "" } else { "s" }
            ),
            Ok(None) => return,
            Err(e) => format!("Printing failed: {e}"),
        };
        let _ = slint::invoke_from_event_loop(move || {
            viewer::with(|app| app.status(message));
        });
    });
}

/// Returns the number of pages printed, or None if the dialog was cancelled.
fn print(
    engine: &mp_engine::Engine,
    doc: mp_engine::DocId,
    sizes: &[(f32, f32)],
    title: &str,
) -> Result<Option<usize>, String> {
    let count = sizes.len().min(u16::MAX as usize) as u16;
    // SAFETY: PRINTDLGW is plain data; zeroed is its documented initial state.
    let mut pd: PRINTDLGW = unsafe { std::mem::zeroed() };
    pd.lStructSize = size_of::<PRINTDLGW>() as u32;
    pd.Flags = PD_RETURNDC | PD_NOSELECTION | PD_USEDEVMODECOPIESANDCOLLATE;
    pd.nMinPage = 1;
    pd.nMaxPage = count;
    pd.nFromPage = 1;
    pd.nToPage = count;
    pd.nCopies = 1;
    // SAFETY: pd is initialised as the API requires.
    let ok = unsafe { PrintDlgW(&mut pd) } != 0;
    // SAFETY: handles returned by PrintDlgW are ours to free; null handles are ignored.
    unsafe {
        GlobalFree(pd.hDevMode);
        GlobalFree(pd.hDevNames);
    }
    if !ok || pd.hDC.is_null() {
        return Ok(None);
    }
    let hdc = pd.hDC;
    let (first, last) = if pd.Flags & PD_PAGENUMS != 0 {
        (
            (pd.nFromPage.max(1) - 1) as usize,
            (pd.nToPage.max(pd.nFromPage).min(count) - 1) as usize,
        )
    } else {
        (0, count as usize - 1)
    };

    // SAFETY: hdc is a valid printer DC until DeleteDC below.
    let (area_w, area_h) = unsafe {
        (
            GetDeviceCaps(hdc, HORZRES as i32),
            GetDeviceCaps(hdc, VERTRES as i32),
        )
    };
    let name: Vec<u16> = title.encode_utf16().chain([0]).collect();
    let info = DOCINFOW {
        cbSize: size_of::<DOCINFOW>() as i32,
        lpszDocName: name.as_ptr(),
        lpszOutput: std::ptr::null(),
        lpszDatatype: std::ptr::null(),
        fwType: 0,
    };
    // SAFETY: valid DC and DOCINFOW whose strings outlive the call.
    if unsafe { StartDocW(hdc, &info) } <= 0 {
        unsafe { DeleteDC(hdc) };
        return Err("the printer did not start the job".into());
    }

    let mut printed = 0;
    let result = (|| {
        for (page, &(w, h)) in sizes.iter().enumerate().take(last + 1).skip(first) {
            // Turn the page to match the paper's orientation.
            let rotation = if (w > h) != (area_w > area_h) { 90 } else { 0 };
            let (pw, ph) = if rotation == 90 { (h, w) } else { (w, h) };
            let fit = (area_w as f32 / pw).min(area_h as f32 / ph);
            let (dest_w, dest_h) = ((pw * fit) as i32, (ph * fit) as i32);
            // `fit` is printer pixels per point; render no finer than MAX_DPI.
            let scale = fit.min(MAX_DPI / 72.0);
            let tile = Tile {
                x: 0,
                y: 0,
                width: (pw * scale).ceil().max(1.0) as i32,
                height: (ph * scale).ceil().max(1.0) as i32,
            };
            let image = engine
                .display_list(doc, page)
                .and_then(|list| mp_engine::render_tile(&list, scale, rotation, tile))
                .map_err(|e| format!("page {}: {e}", page + 1))?;
            let bits = to_dib(&image.rgb, image.width as usize, image.height as usize);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: image.width as i32,
                    biHeight: -(image.height as i32),
                    biPlanes: 1,
                    biBitCount: 24,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            // SAFETY: valid DC; bits match the header's size and 4-byte row alignment.
            unsafe {
                if StartPage(hdc) <= 0 {
                    return Err("the printer rejected a page".to_string());
                }
                StretchDIBits(
                    hdc,
                    (area_w - dest_w) / 2,
                    (area_h - dest_h) / 2,
                    dest_w,
                    dest_h,
                    0,
                    0,
                    image.width as i32,
                    image.height as i32,
                    bits.as_ptr().cast(),
                    &bmi,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
                if EndPage(hdc) <= 0 {
                    return Err("the printer rejected a page".to_string());
                }
            }
            printed += 1;
        }
        Ok(())
    })();
    // SAFETY: hdc is valid; the job is ended or aborted exactly once.
    unsafe {
        if result.is_ok() {
            EndDoc(hdc);
        } else {
            AbortDoc(hdc);
        }
        DeleteDC(hdc);
    }
    result.map(|()| Some(printed))
}

/// Packed RGB rows -> BGR rows padded to 4 bytes, as GDI wants.
fn to_dib(rgb: &[u8], width: usize, height: usize) -> Vec<u8> {
    let stride = (width * 3).div_ceil(4) * 4;
    let mut out = vec![0u8; stride * height];
    for (src, dst) in rgb
        .chunks_exact(width * 3)
        .zip(out.chunks_exact_mut(stride))
    {
        for (s, d) in src
            .as_chunks::<3>()
            .0
            .iter()
            .zip(dst.as_chunks_mut::<3>().0)
        {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
        }
    }
    out
}
