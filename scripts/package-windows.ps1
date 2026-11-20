# Packages a release build of Moonglass for Windows (spec 4.5): the single
# .exe and a zip with it, the readme, the license and the third-party
# notices, in dist\.
#
#   cargo build --release -p moonglass-launcher
#   bash scripts/third-party-notices.sh     (needs cargo-about)
#   powershell -File scripts\package-windows.ps1
#
# Without the notices the zip is made anyway, with a warning; on CI (the
# CI variable set) that is an error.
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$name = "moonglass-$version-windows-x86_64"
$bin = 'target\release\moonglass.exe'
if (-not (Test-Path $bin)) { throw 'build first: cargo build --release -p moonglass-launcher' }
$notices = 'dist\THIRD-PARTY-NOTICES.html'
if (-not (Test-Path $notices)) {
    if ($env:CI) { throw 'third-party notices missing: run scripts/third-party-notices.sh' }
    Write-Warning 'third-party notices missing: run scripts/third-party-notices.sh'
}

New-Item -ItemType Directory -Force dist | Out-Null
$staging = Join-Path dist $name
if (Test-Path $staging) { Remove-Item -Recurse -Force $staging }
New-Item -ItemType Directory $staging | Out-Null

Copy-Item $bin (Join-Path dist "$name.exe")
Copy-Item $bin (Join-Path $staging 'moonglass.exe')
Copy-Item README.md, LICENSE $staging
if (Test-Path $notices) { Copy-Item $notices $staging }
$zip = Join-Path dist "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path (Join-Path $staging '*') -DestinationPath $zip
Remove-Item -Recurse -Force $staging

Write-Output (Join-Path dist "$name.exe")
Write-Output $zip
