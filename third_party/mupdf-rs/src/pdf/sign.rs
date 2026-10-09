//! Digital signatures through `shim/sign.c`: a signer on Windows CryptoAPI for a certificate
//! in the user's store or a .pfx file, and checks of signed fields.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::os::windows::ffi::OsStrExt;
use std::ptr::NonNull;

use mupdf_sys::{fz_context, pdf_annot};

use crate::pdf::PdfWidget;
use crate::pdf::document::journal_call;
use crate::{Error, context};

/// Opaque `pdf_pkcs7_signer`.
#[repr(C)]
struct RawSigner {
    _private: [u8; 0],
}

#[repr(C)]
struct RawCheck {
    digest: c_int,
    certificate: c_int,
    changed: c_int,
    signer: [c_char; 256],
}

unsafe extern "C" {
    fn mp_signer_from_store(
        hash: *const u8,
        tsa: *const u16,
        out: *mut *mut RawSigner,
        err: *mut *const c_char,
    ) -> c_int;
    fn mp_signer_from_pfx(
        data: *const u8,
        len: usize,
        password: *const u16,
        tsa: *const u16,
        out: *mut *mut RawSigner,
        err: *mut *const c_char,
    ) -> c_int;
    fn mp_signer_drop(signer: *mut RawSigner);
    fn mp_signer_name(signer: *mut RawSigner, name: *mut c_char, size: usize);
    fn mp_list_certificates(
        each: unsafe extern "C" fn(
            *mut c_void,
            *const u8,
            *const c_char,
            *const c_char,
            *const c_char,
        ),
        arg: *mut c_void,
    );
    fn mp_pdf_sign_signature(
        ctx: *mut fz_context,
        widget: *mut pdf_annot,
        signer: *mut RawSigner,
        flags: c_int,
        reason: *const c_char,
        location: *const c_char,
        err: *mut *const c_char,
    ) -> c_int;
    fn mp_pdf_check_signature(
        ctx: *mut fz_context,
        widget: *mut pdf_annot,
        check: *mut RawCheck,
        err: *mut *const c_char,
    ) -> c_int;
}

/// What the signed field shows: labels, the signer's distinguished name, the date, the name as
/// text and the name written large.
pub const SIGNATURE_APPEARANCE: i32 = 1 | 2 | 4 | 8 | 16;

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

/// A certificate in the user's personal store that can sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    /// SHA-1 thumbprint, which finds it in the store again.
    pub thumbprint: [u8; 20],
    pub name: String,
    pub issuer: String,
    /// YYYY-MM-DD.
    pub expires: String,
}

unsafe extern "C" fn collect(
    arg: *mut c_void,
    hash: *const u8,
    name: *const c_char,
    issuer: *const c_char,
    expires: *const c_char,
) {
    // SAFETY: `arg` is the Vec passed to mp_list_certificates; the strings are NUL-terminated
    // and `hash` points at 20 bytes, all valid during the call.
    unsafe {
        let list = &mut *(arg as *mut Vec<Certificate>);
        let text = |p: *const c_char| CStr::from_ptr(p).to_string_lossy().into_owned();
        let mut thumbprint = [0; 20];
        thumbprint.copy_from_slice(std::slice::from_raw_parts(hash, 20));
        list.push(Certificate {
            thumbprint,
            name: text(name),
            issuer: text(issuer),
            expires: text(expires),
        });
    }
}

/// The certificates with private keys in the user's personal store.
pub fn certificates() -> Vec<Certificate> {
    let mut list: Vec<Certificate> = Vec::new();
    // SAFETY: `collect` only runs during the call, while `list` is alive.
    unsafe { mp_list_certificates(collect, &mut list as *mut _ as *mut c_void) };
    list
}

pub struct PdfSigner {
    inner: NonNull<RawSigner>,
}

// The signer holds CryptoAPI handles, which any thread may use.
unsafe impl Send for PdfSigner {}

fn signer_call(f: impl FnOnce(*mut *mut RawSigner, *mut *const c_char) -> c_int) -> Result<PdfSigner, Error> {
    let mut out = std::ptr::null_mut();
    journal_call(|err| f(&mut out, err))?;
    NonNull::new(out)
        .map(|inner| PdfSigner { inner })
        .ok_or(Error::UnexpectedNullPtr)
}

impl PdfSigner {
    /// The certificate in the user's personal store with this thumbprint; with a timestamp
    /// authority's URL, signatures are timestamped by it.
    pub fn from_store(thumbprint: &[u8; 20], tsa: Option<&str>) -> Result<Self, Error> {
        let tsa = tsa.map(wide);
        let tsa = tsa.as_ref().map_or(std::ptr::null(), |t| t.as_ptr());
        // SAFETY: the thumbprint and URL outlive the call.
        signer_call(|out, err| unsafe { mp_signer_from_store(thumbprint.as_ptr(), tsa, out, err) })
    }

    /// The certificate with a private key in the bytes of a .pfx or .p12 file.
    pub fn from_pfx(data: &[u8], password: &str, tsa: Option<&str>) -> Result<Self, Error> {
        let password = wide(password);
        let tsa = tsa.map(wide);
        let tsa = tsa.as_ref().map_or(std::ptr::null(), |t| t.as_ptr());
        // SAFETY: the data, password and URL outlive the call.
        signer_call(|out, err| unsafe {
            mp_signer_from_pfx(data.as_ptr(), data.len(), password.as_ptr(), tsa, out, err)
        })
    }

    /// The certificate's subject, as Windows shows it.
    pub fn name(&self) -> String {
        let mut buf = [0 as c_char; 512];
        // SAFETY: the buffer is as large as said.
        unsafe { mp_signer_name(self.inner.as_ptr(), buf.as_mut_ptr(), buf.len()) };
        // SAFETY: mp_signer_name always NUL-terminates.
        unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for PdfSigner {
    fn drop(&mut self) {
        // SAFETY: MuPDF keeps its own reference while a signature waits to be written.
        unsafe { mp_signer_drop(self.inner.as_ptr()) }
    }
}

/// MuPDF's `pdf_signature_error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureError {
    Okay,
    NoSignatures,
    NoCertificate,
    DigestFailure,
    SelfSigned,
    SelfSignedInChain,
    NotTrusted,
    NotSigned,
    Unknown,
}

impl SignatureError {
    fn from_raw(v: c_int) -> Self {
        use SignatureError::*;
        [
            Okay,
            NoSignatures,
            NoCertificate,
            DigestFailure,
            SelfSigned,
            SelfSignedInChain,
            NotTrusted,
            NotSigned,
        ]
        .get(v as usize)
        .copied()
        .unwrap_or(Unknown)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureCheck {
    /// Whether the signed bytes are as they were signed.
    pub digest: SignatureError,
    /// Whether Windows trusts the signer's certificate.
    pub certificate: SignatureError,
    /// The file was added to after this signature.
    pub changed: bool,
    /// The certificate's common name.
    pub signer: String,
}

impl PdfWidget {
    /// Signs this signature field. The signature is computed when the document is next saved.
    pub fn sign(
        &mut self,
        signer: &PdfSigner,
        flags: i32,
        reason: &str,
        location: &str,
    ) -> Result<(), Error> {
        self.annotation().ensure_attached()?;
        let (reason, location) = (CString::new(reason)?, CString::new(location)?);
        let widget = self.annotation().inner.as_ptr();
        // SAFETY: the widget is attached; the strings outlive the call, and MuPDF keeps its own
        // reference to the signer.
        journal_call(|err| unsafe {
            mp_pdf_sign_signature(
                context(),
                widget,
                signer.inner.as_ptr(),
                flags,
                reason.as_ptr(),
                location.as_ptr(),
                err,
            )
        })
    }

    /// Checks this signed field against the document's saved bytes.
    pub fn check_signature(&self) -> Result<SignatureCheck, Error> {
        self.annotation().ensure_attached()?;
        let mut raw = RawCheck {
            digest: 0,
            certificate: 0,
            changed: 0,
            signer: [0; 256],
        };
        let widget = self.annotation().inner.as_ptr();
        // SAFETY: the widget is attached and `raw` outlives the call.
        journal_call(|err| unsafe { mp_pdf_check_signature(context(), widget, &mut raw, err) })?;
        Ok(SignatureCheck {
            digest: SignatureError::from_raw(raw.digest),
            certificate: SignatureError::from_raw(raw.certificate),
            changed: raw.changed != 0,
            // SAFETY: the shim leaves the last byte zero.
            signer: unsafe { CStr::from_ptr(raw.signer.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
        })
    }
}

impl crate::pdf::PdfDocument {
    /// Runs `f` with its edits kept in the incremental section that holds a signature still
    /// to be written. MuPDF otherwise starts a new section for any edit after signing, which
    /// the signature would not cover; edits that belong to the signature itself, such as its
    /// certification entries, must go in its section.
    pub fn amend_signature<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let raw = self.as_raw();
        // SAFETY: `raw` is this open document; the flag only steers where edits are stored.
        let before = unsafe { std::mem::replace(&mut (*raw).disallow_new_increments, 1) };
        let result = f(self);
        // SAFETY: as above.
        unsafe { (*raw).disallow_new_increments = before };
        result
    }
}
