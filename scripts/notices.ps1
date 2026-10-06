# Regenerates THIRD-PARTY-NOTICES.md from the Cargo dependency graph.
# Requires: cargo install cargo-about --locked
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    cargo about generate --config about.toml --workspace --output-file THIRD-PARTY-NOTICES.md scripts/about.hbs
} finally {
    Pop-Location
}
