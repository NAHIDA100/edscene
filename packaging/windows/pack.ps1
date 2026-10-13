param(
    [string]$Assets = '',
    [string]$Background = '',
    [string]$Out = 'D:\works\edscene-dist',
    [string]$Exe = '',
    [string]$Iscc = '',
    [string]$Version = ''
)

$ErrorActionPreference = 'Stop'
$src = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if ($Version -eq '') {
    $match = [regex]::Match((Get-Content (Join-Path $src 'Cargo.toml') -Raw), '(?m)^version\s*=\s*"([^"]+)"')
    if (-not $match.Success) { throw 'Cannot read workspace version' }
    $Version = $match.Groups[1].Value
}
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'Version must be major.minor.patch' }
if ($Exe -eq '') { $Exe = Join-Path $src 'target\release\edscene.exe' }
if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) { throw "Build eds-app first: $Exe" }
if ($Iscc -eq '') {
    $compiler = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($compiler) { $Iscc = $compiler.Source }
    else { $Iscc = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe' }
}
if (-not (Test-Path -LiteralPath $Iscc -PathType Leaf)) { throw 'Specify -Iscc with the path to Inno Setup 6 ISCC.exe' }
if ($Assets -and -not (Test-Path -LiteralPath $Assets -PathType Container)) { throw "Missing assets: $Assets" }
if ($Background -and -not (Test-Path -LiteralPath $Background -PathType Leaf)) { throw "Missing background: $Background" }

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path -LiteralPath $Out).Path
$stage = Join-Path $Out ('pkg\inno-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage -Force | Out-Null
try {
    Copy-Item -LiteralPath $Exe -Destination (Join-Path $stage 'edscene.exe')
    foreach ($file in 'README.md','README.en.md','CHANGELOG.md','LICENSE-code','LICENSE-content','NOTICE') {
        Copy-Item -LiteralPath (Join-Path $src $file) -Destination $stage
    }
    Copy-Item -LiteralPath (Join-Path $src 'docs') -Destination $stage -Recurse
    $code = Join-Path $Out "edscene-$Version-windows-x64.zip"
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $code -CompressionLevel Optimal -Force

    if ($Assets) {
        & python (Join-Path $PSScriptRoot 'pack_sounds.py') $Assets (Join-Path $stage 'sounds')
        if ($LASTEXITCODE -ne 0) { throw 'Sound pack preparation failed' }
    }
    if ($Background) {
        Copy-Item -LiteralPath $Background -Destination (Join-Path $stage 'background.jpg')
    }
    & $Iscc /Qp "/DStageDir=$stage" "/DReleaseDir=$Out" "/DAppVersion=$Version" (Join-Path $PSScriptRoot 'setup.iss')
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed: $LASTEXITCODE" }
    $installer = Join-Path $Out "edscene-setup-$Version-windows-x64.exe"
    if (-not (Test-Path -LiteralPath $installer)) { throw 'Installer output missing' }
    $full = Join-Path $Out "edscene-setup-$Version-windows-x64.zip"
    Compress-Archive -LiteralPath $installer, (Join-Path $PSScriptRoot 'installer-notice.txt') -DestinationPath $full -CompressionLevel Optimal -Force
    $sums = foreach ($file in $code, $full, $installer) {
        $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
        Write-Host "$hash  $((Get-Item -LiteralPath $file).Length) bytes  $(Split-Path $file -Leaf)"
        "$hash *$(Split-Path $file -Leaf)"
    }
    (($sums -join "`n") + "`n") | Set-Content -LiteralPath (Join-Path $Out "SHA256SUMS-$Version-windows.txt") -Encoding ascii -NoNewline
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force
}
