# Packages a release build of Moonglass for Windows (spec 4.5): the single
# .exe, a zip with it, the readme, the license and the third-party
# notices, and an installer (packaging\windows\moonglass.iss), in dist\.
#
#   cargo build --release -p moonglass-launcher
#   bash scripts/third-party-notices.sh     (needs cargo-about)
#   powershell -File scripts\package-windows.ps1
#
# The installer needs Inno Setup 6 (its ISCC.exe on PATH, in the usual
# folders, or named by the ISCC variable). Without the notices or Inno
# Setup the rest is made anyway, with a warning; on CI (the CI variable
# set) both are errors.
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

# Installer.
$iscc = @(
    $env:ISCC,
    (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source,
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) {
    if ($env:CI) { throw 'Inno Setup 6 not found: the installer cannot be built' }
    Write-Warning 'Inno Setup 6 not found: installer skipped'
    return
}
$root = (Resolve-Path .).Path
# Windows wants four numbers in a file's version: 0.1.0-beta.2 is 0.1.0.0.
$numeric = ($version -split '[-+]')[0] + '.0'
& $iscc /Qp "/DRoot=$root" "/DAppVersion=$version" "/DNumericVersion=$numeric" `
    "/DOutputDir=$root\dist" "/DOutputName=$name-setup" packaging\windows\moonglass.iss
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed ($LASTEXITCODE)" }
Write-Output (Join-Path dist "$name-setup.exe")
