# Releasing

## Once

**Update key.** micropdf trusts an update only if its manifest is signed with the release key.
The public half is `KEY` in `app/src/update.rs`. The private half was made with
`cargo run -p micropdf --example sign_update -- keygen <file>`. Keep it out of the repository, in
a password manager and as the GitHub Actions secret `UPDATE_KEY` (the file's bytes, base64). Lose
it and installed copies can no longer update themselves: make a new key, put its public half in
`KEY`, and ask users to install the next release by hand.

**Code-signing certificate.** Unsigned executables trip SmartScreen ("Windows protected your
PC"). Either:

- [SignPath Foundation](https://signpath.org), free for open-source projects: apply with the
  repository, then sign in CI with their GitHub action; or
- [Azure Trusted Signing](https://learn.microsoft.com/azure/trusted-signing/), about $10 a month,
  for individuals and companies with a verified identity; or
- any code-signing certificate in your Windows certificate store: `scripts/release.ps1` signs
  with it when `MICROPDF_CERT_THUMBPRINT` holds its SHA-1 thumbprint.

Signatures carry an RFC 3161 timestamp, so they stay valid after the certificate expires.

**Store accounts.** A Chrome Web Store developer account (one-time $5 fee) and a Microsoft Partner
Center account for Edge Add-ons (free).

## Each release

1. Set the version in `Cargo.toml` (`[workspace.package]`), `extension/package.json` and
   `extension/public/manifest.json`, and commit.
2. `cargo test --workspace`, and in `extension/`, `npm run check`, `npm test` and `npm run e2e`.
3. Build: `powershell -File scripts\release.ps1` with `MICROPDF_UPDATE_KEY` set to the key
   file, and `MICROPDF_CERT_THUMBPRINT` if signing locally. `dist\` then holds
   `micropdf-<version>-windows-x64.zip`, `latest.json` and `latest.json.sig`.
4. Tag `v<version>`, push the tag, and make a GitHub release from it with those three files.
   Installed copies find it through `releases/latest/download/latest.json`.
5. Extension: `npm run build` in `extension/`, then zip the contents of `extension/dist`.
   - The stores do not accept the manifest's `key`; delete that line from the zipped
     `manifest.json`. Each store then gives the extension its own ID; add both IDs to
     `EXTENSION_IDS` in `crates/mp-bridge/src/main.rs` (once) so the bridge accepts them.
   - Upload to the Chrome Web Store and to Edge Add-ons with the texts in
     [store/listing.md](store/listing.md), and the privacy policy at
     <https://github.com/pachisiav11/micropdf/blob/main/docs/privacy.md>.

## Checks before announcing

- Install from the zip on a clean Windows user: Start menu entry, "Open with" for PDFs,
  Settings > Apps entry, and uninstall from there.
- "Check for updates" in the previous release offers the new one, and installing it restarts
  into the new version.
- The extension from the store opens a PDF in its viewer and hands it to the app.
