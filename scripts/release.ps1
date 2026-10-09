# Builds a release into dist\: micropdf.exe and micropdf-bridge.exe, signed when a code-signing
# certificate's thumbprint is given, zipped with the license, the notices and Install.cmd as
# micropdf-<version>-windows-x64.zip, and the update manifest latest.json, signed when the update
# key is given. See RELEASING.md.
param(
    [string]$CertificateThumbprint = $env:MICROPDF_CERT_THUMBPRINT,
    [string]$UpdateKey = $env:MICROPDF_UPDATE_KEY
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value

cargo build --release -p micropdf -p mp-bridge
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$exes = 'target\release\micropdf.exe', 'target\release\micropdf-bridge.exe'
if ($CertificateThumbprint) {
    $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
        Sort-Object FullName | Select-Object -Last 1
    & $signtool.FullName sign /sha1 $CertificateThumbprint /fd sha256 `
        /tr http://timestamp.digicert.com /td sha256 $exes
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
}

$name = "micropdf-$version-windows-x64"
$stage = "dist\$name"
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item -Force ($exes + 'LICENSE', 'THIRD-PARTY-NOTICES.md') $stage
Set-Content -Encoding ascii "$stage\Install.cmd" '@start "" "%~dp0micropdf.exe" --install'
$zip = "dist\$name.zip"
Compress-Archive -Force "$stage\*" $zip

$manifest = [ordered]@{
    version = $version
    url = "https://github.com/pachisiav11/micropdf/releases/download/v$version/$name.zip"
    sha256 = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
} | ConvertTo-Json -Compress
[IO.File]::WriteAllText("$root\dist\latest.json", $manifest)
if ($UpdateKey) {
    cargo run --release -p micropdf --example sign_update -- sign $UpdateKey dist\latest.json
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
}
Write-Output "Built $zip"
