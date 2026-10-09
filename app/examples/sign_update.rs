//! Makes the update key and signs update manifests with it; scripts/release.ps1 runs `sign`.
//!
//!     cargo run -p micropdf --example sign_update -- keygen <key file>
//!     cargo run -p micropdf --example sign_update -- sign <key file> <manifest>
//!
//! `keygen` writes the private key, which stays secret and out of the repository, and prints
//! the public half for `KEY` in app/src/update.rs. `sign` writes `<manifest>.sig`: the raw
//! 64-byte ECDSA P-256 signature of the manifest's SHA-256, which the app checks with Windows
//! CNG before it trusts the manifest.

use std::ptr::{null, null_mut};

use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_ECCPRIVATE_BLOB, BCRYPT_ECDSA_P256_ALGORITHM, BCRYPT_KEY_HANDLE,
    BCRYPT_SHA256_ALGORITHM, BCryptCloseAlgorithmProvider, BCryptExportKey, BCryptFinalizeKeyPair,
    BCryptGenerateKeyPair, BCryptHash, BCryptImportKeyPair, BCryptOpenAlgorithmProvider,
    BCryptSignHash,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args[..] {
        ["keygen", key] => keygen(key),
        ["sign", key, manifest] => sign(key, manifest),
        _ => {
            eprintln!("usage: sign_update keygen <key file> | sign <key file> <manifest>");
            std::process::exit(2);
        }
    }
}

fn check(status: i32, what: &str) {
    if status != 0 {
        eprintln!("{what} failed: {status:#x}");
        std::process::exit(1);
    }
}

fn provider(name: *const u16) -> BCRYPT_ALG_HANDLE {
    let mut algorithm = null_mut();
    // SAFETY: `name` is one of CNG's NUL-terminated algorithm names.
    check(
        unsafe { BCryptOpenAlgorithmProvider(&mut algorithm, name, null(), 0) },
        "Opening the algorithm",
    );
    algorithm
}

fn keygen(path: &str) {
    if std::path::Path::new(path).exists() {
        eprintln!("{path} exists; not replacing a key");
        std::process::exit(1);
    }
    let algorithm = provider(BCRYPT_ECDSA_P256_ALGORITHM);
    let mut key: BCRYPT_KEY_HANDLE = null_mut();
    let mut blob = vec![0u8; 104];
    let mut len = 0;
    // SAFETY: the handles come from CNG; `blob` holds the 8-byte header and X, Y and d.
    unsafe {
        check(
            BCryptGenerateKeyPair(algorithm, &mut key, 256, 0),
            "Making the key",
        );
        check(BCryptFinalizeKeyPair(key, 0), "Making the key");
        check(
            BCryptExportKey(
                key,
                null_mut(),
                BCRYPT_ECCPRIVATE_BLOB,
                blob.as_mut_ptr(),
                blob.len() as u32,
                &mut len,
                0,
            ),
            "Exporting the key",
        );
        BCryptCloseAlgorithmProvider(algorithm, 0);
    }
    std::fs::write(path, &blob[..len as usize]).expect("could not write the key");
    let public: Vec<String> = blob[8..72].iter().map(|b| format!("{b:#04x}")).collect();
    println!("const KEY: [u8; 64] = [{}];", public.join(", "));
}

fn sign(key_path: &str, manifest: &str) {
    let blob = std::fs::read(key_path).expect("could not read the key");
    let data = std::fs::read(manifest).expect("could not read the manifest");
    let (sha, ecdsa) = (
        provider(BCRYPT_SHA256_ALGORITHM),
        provider(BCRYPT_ECDSA_P256_ALGORITHM),
    );
    let mut digest = [0u8; 32];
    let mut key: BCRYPT_KEY_HANDLE = null_mut();
    let mut signature = [0u8; 64];
    let mut len = 0;
    // SAFETY: the buffers hold the lengths passed with them; the handles come from CNG.
    unsafe {
        check(
            BCryptHash(
                sha,
                null(),
                0,
                data.as_ptr(),
                data.len() as u32,
                digest.as_mut_ptr(),
                32,
            ),
            "Hashing",
        );
        check(
            BCryptImportKeyPair(
                ecdsa,
                null_mut(),
                BCRYPT_ECCPRIVATE_BLOB,
                &mut key,
                blob.as_ptr(),
                blob.len() as u32,
                0,
            ),
            "Reading the key",
        );
        check(
            BCryptSignHash(
                key,
                null(),
                digest.as_ptr(),
                32,
                signature.as_mut_ptr(),
                64,
                &mut len,
                0,
            ),
            "Signing",
        );
        BCryptCloseAlgorithmProvider(sha, 0);
        BCryptCloseAlgorithmProvider(ecdsa, 0);
    }
    std::fs::write(format!("{manifest}.sig"), &signature[..len as usize])
        .expect("could not write the signature");
    println!("Signed {manifest}");
}
