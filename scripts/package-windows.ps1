# Packages a release build of Moonglass for Windows (spec 4.5): the single
# .exe and a zip with it and the readme, in dist\.
#
#   cargo build --release -p moonglass-launcher
#   powershell -File scripts\package-windows.ps1
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$name = "moonglass-$version-windows-x86_64"
$bin = 'target\release\moonglass.exe'
if (-not (Test-Path $bin)) { throw 'build first: cargo build --release -p moonglass-launcher' }

New-Item -ItemType Directory -Force dist | Out-Null
$staging = Join-Path dist $name
if (Test-Path $staging) { Remove-Item -Recurse -Force $staging }
New-Item -ItemType Directory $staging | Out-Null

Copy-Item $bin (Join-Path dist "$name.exe")
Copy-Item $bin (Join-Path $staging 'moonglass.exe')
Copy-Item README.md $staging
$zip = Join-Path dist "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path (Join-Path $staging '*') -DestinationPath $zip
Remove-Item -Recurse -Force $staging

Write-Output (Join-Path dist "$name.exe")
Write-Output $zip
