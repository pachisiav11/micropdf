//! Signing with a certificate from the Windows store, a smart card or a .pfx file, and the
//! rows of the Signed panel.

use mp_engine::{
    DocId, SignField, SignWith, Signature, Signing, Trust, certificates, readable_date,
};

use crate::tools::{self, Done, Form, check, choice, password, text};
use crate::viewer::{self, App};
use crate::{FormField, SignatureRow};

/// Asks how to sign `field`.
pub fn open(app: &mut App, field: SignField) {
    let names: Vec<String> = certificates()
        .iter()
        .map(|c| format!("{} ({}, until {})", c.name, c.issuer, c.expires))
        .chain(["From a .pfx or .p12 file\u{2026}".to_owned()])
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    app.show_form(
        Form::Sign(field),
        "Sign with a certificate",
        "The file is saved as you sign. A smart card or token may ask for its PIN.",
        "Sign",
        vec![
            choice("Certificate", &names, 0),
            password("Password of the file"),
            text("Reason", ""),
            text("Location", ""),
            text("Timestamp server", ""),
            check("Certify: later, allow only form filling and signing", false),
        ],
    );
}

pub fn run(app: &mut App, field: SignField, f: &[FormField]) -> Done {
    let Some((doc, path, ..)) = app.reading() else {
        return Ok(());
    };
    let mut how = Signing {
        with: SignWith::Store([0; 20]),
        field,
        reason: f[2].text.trim().to_owned(),
        location: f[3].text.trim().to_owned(),
        tsa: Some(f[4].text.trim().to_owned()).filter(|t| !t.is_empty()),
        certify: f[5].checked,
    };
    if let Some(cert) = certificates().get(f[0].index.max(0) as usize) {
        how.with = SignWith::Store(cert.thumbprint);
        sign(app, doc, how);
        return Ok(());
    }
    let password = f[1].text.to_string();
    let ids = ("Digital IDs", &["pfx", "p12"][..]);
    tools::pick_files(
        "Choose your digital ID",
        Some(path),
        false,
        ids,
        move |files| {
            let data = std::fs::read(&files[0]);
            let _ = slint::invoke_from_event_loop(move || {
                viewer::with(|app| match data {
                    Ok(data) => {
                        how.with = SignWith::Pfx { data, password };
                        sign(app, doc, how);
                    }
                    Err(e) => app.message("Could not read the digital ID", e.to_string()),
                });
            });
        },
    );
    Ok(())
}

fn sign(app: &mut App, doc: DocId, how: Signing) {
    match app.engine().sign(doc, how) {
        Ok(()) => app.save_signed(doc),
        Err(e) => app.message("Could not sign", e.to_string()),
    }
}

pub fn rows(list: &[Signature]) -> Vec<SignatureRow> {
    list.iter()
        .map(|s| {
            let (state, mut status) = match (s.signed, s.intact, s.trust) {
                (false, ..) => (0, "Not signed. Click the field to sign it.".to_owned()),
                (_, false, _) => (3, "Invalid: the signed content was changed.".to_owned()),
                (_, _, Trust::Trusted) => (1, "Valid.".to_owned()),
                (_, _, Trust::Unknown) => (
                    2,
                    "Valid, but Windows does not know the signer's certificate.".to_owned(),
                ),
                (_, _, Trust::Untrusted) => (
                    3,
                    "The signer's certificate is expired, revoked or broken.".to_owned(),
                ),
            };
            if s.certifies {
                status.insert_str(0, "Certifies the document. ");
            }
            if s.signed && s.intact && s.changed_after {
                status.push_str(" The file was added to after signing.");
            }
            let mut detail = vec![format!("Page {}", s.page + 1)];
            if let Some(date) = readable_date(&s.date) {
                detail.push(format!("Signed {date}"));
            }
            for (label, value) in [("Reason", &s.reason), ("Location", &s.location)] {
                if !value.is_empty() {
                    detail.push(format!("{label}: {value}"));
                }
            }
            SignatureRow {
                name: if s.signer.is_empty() {
                    &s.name
                } else {
                    &s.signer
                }
                .into(),
                status: status.into(),
                detail: detail.join("\n").into(),
                state,
            }
        })
        .collect()
}
