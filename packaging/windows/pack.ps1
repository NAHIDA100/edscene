# Assemble the Windows release archives from a built tree.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File packaging\windows\pack.ps1 `
#       -Assets "D:\works\eds" -Background "C:\path\to\picture.jpg" -Out "D:\works\edscene-dist"
#
# Two archives come out:
#   edscene-<v>-windows-x64.zip        code + docs only (what CI can publish)
#   edscene-setup-<v>-windows-x64.zip  the above plus sounds\ and background.jpg
#
# The assets are deliberately NOT in the repository: this script only ever
# copies them from a directory you point at, so the fan artwork stays out of
# version control while the download still works with zero setup.
#
# Keep this file ASCII-only: Windows PowerShell 5.1 reads .ps1 as ANSI.

param(
    [string]$Assets = '',
    [string]$Background = '',
    [string]$Out = 'D:\works\edscene-dist',
    [string]$Exe = '',
    [string]$Version = '0.1.0'
)

$ErrorActionPreference = 'Stop'
$src = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$pkg = Join-Path $Out "pkg\edscene-$Version-windows-x64"

if ($Exe -eq '') { $exe = Join-Path $src 'target\release\edscene.exe' } else { $exe = $Exe }
if (-not (Test-Path $exe)) { throw "先 cargo build --release：找不到 $exe" }

New-Item -ItemType Directory -Force -Path $pkg | Out-Null
Copy-Item $exe (Join-Path $pkg 'edscene.exe') -Force
foreach ($f in 'README.md', 'README.en.md', 'CHANGELOG.md', 'LICENSE-code', 'LICENSE-content', 'NOTICE') {
    Copy-Item (Join-Path $src $f) $pkg -Force
}
New-Item -ItemType Directory -Force -Path (Join-Path $pkg 'docs') | Out-Null
Copy-Item (Join-Path $src 'docs\*.md') (Join-Path $pkg 'docs') -Force
Copy-Item (Join-Path $PSScriptRoot 'install.ps1') (Join-Path $pkg 'install.ps1') -Force
Copy-Item (Join-Path $PSScriptRoot 'setup.bat') (Join-Path $pkg 'setup.bat') -Force

if ($Assets -ne '') {
    python (Join-Path $PSScriptRoot 'pack_sounds.py') $Assets (Join-Path $pkg 'sounds')
    if ($LASTEXITCODE -ne 0) { throw 'pack_sounds.py failed' }
}
if ($Background -ne '') { Copy-Item $Background (Join-Path $pkg 'background.jpg') -Force }

$code = Join-Path $Out "edscene-$Version-windows-x64.zip"
$full = Join-Path $Out "edscene-setup-$Version-windows-x64.zip"
Remove-Item $code, $full -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $pkg 'edscene.exe'), (Join-Path $pkg 'docs'),
    (Join-Path $pkg '*.md'), (Join-Path $pkg 'LICENSE-code'), (Join-Path $pkg 'LICENSE-content'),
    (Join-Path $pkg 'NOTICE'), (Join-Path $pkg 'install.ps1'), (Join-Path $pkg 'setup.bat') `
    -DestinationPath $code -CompressionLevel Optimal
Compress-Archive -Path (Join-Path $pkg '*') -DestinationPath $full -CompressionLevel Optimal

$sums = @()
foreach ($z in $code, $full) {
    $h = (Get-FileHash $z -Algorithm SHA256).Hash.ToLower()
    $sums += "$h *$(Split-Path $z -Leaf)"
    "{0}  {1,9} bytes  {2}" -f $h, (Get-Item $z).Length, (Split-Path $z -Leaf)
}
$sums += "$( (Get-FileHash (Join-Path $pkg 'edscene.exe') -Algorithm SHA256).Hash.ToLower() ) *edscene.exe"
(($sums -join "`n") + "`n") | Set-Content (Join-Path $Out 'SHA256SUMS.txt') -NoNewline -Encoding ascii
"wrote $(Join-Path $Out 'SHA256SUMS.txt')"
