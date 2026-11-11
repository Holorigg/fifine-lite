# Builds the release files into dist\: the installer, the portable zip and SHA256SUMS.txt.
# Needs Rust and Inno Setup 6 (winget install JRSoftware.InnoSetup).
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "FIFINE Lite $version"

cargo test --release --quiet
if ($LASTEXITCODE) { throw 'tests failed' }
cargo build --release
if ($LASTEXITCODE) { throw 'build failed' }

$iscc = @(
    (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source,
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found' }

$dist = Join-Path $root 'dist'
Remove-Item $dist -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $dist | Out-Null

& $iscc /Q "/DAppVersion=$version" installer\fifine-lite.iss
if ($LASTEXITCODE) { throw 'installer build failed' }

# Portable: the executable alone runs anywhere; settings still go to %APPDATA%\fifine-lite.
$stage = Join-Path $dist 'portable'
New-Item -ItemType Directory $stage | Out-Null
Copy-Item target\release\fifine-lite.exe $stage
Copy-Item LICENSE (Join-Path $stage 'LICENSE.txt')
Compress-Archive -Path "$stage\*" -DestinationPath (Join-Path $dist "FIFINE-Lite-$version-portable-x64.zip")
Remove-Item $stage -Recurse -Force

Push-Location $dist
Get-ChildItem -File | Where-Object Name -ne 'SHA256SUMS.txt' | ForEach-Object {
    '{0}  {1}' -f (Get-FileHash $_.Name -Algorithm SHA256).Hash.ToLower(), $_.Name
} | Set-Content SHA256SUMS.txt -Encoding ascii
Pop-Location
Get-ChildItem $dist | Format-Table Name, Length -AutoSize
