# Capture a window of a running process to a PNG, without any browser.
#
#   powershell -NoProfile -File scripts\shot.ps1 -ProcessName spike -OutFile shot.png
#
# Tries PrintWindow (works even when the window is behind others) and falls
# back to a screen grab of the window rectangle, which is what a user would
# see. Both paths are reported so a black PNG can be told apart from a
# capture that never happened.
param(
    [Parameter(Mandatory = $true)][string]$ProcessName,
    [string]$OutFile = "shot.png",
    [int]$TimeoutSeconds = 25
)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

$signature = @'
using System;
using System.Runtime.InteropServices;
public static class Win {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr hWnd, IntPtr hDC);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
'@
Add-Type $signature
# Without this, GetWindowRect returns virtualized (DPI-scaled) coordinates and the
# capture silently misses the right/bottom of a per-monitor-aware window.
[void][Win]::SetProcessDPIAware()

$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
$proc = $null
while ((Get-Date) -lt $deadline) {
    $candidates = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue |
        Where-Object { $_.MainWindowHandle -ne 0 }
    if ($candidates) { $proc = $candidates[0]; break }
    Start-Sleep -Milliseconds 250
}
if (-not $proc) {
    Write-Error "no process '$ProcessName' with a visible main window within ${TimeoutSeconds}s"
    exit 2
}

$hwnd = $proc.MainWindowHandle
$rect = New-Object Win+RECT
[void][Win]::GetWindowRect($hwnd, [ref]$rect)
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
if ($w -le 0 -or $h -le 0) { Write-Error "window has no size ($w x $h)"; exit 3 }

$bmp = New-Object System.Drawing.Bitmap($w, $h, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
$ok = [Win]::PrintWindow($hwnd, $hdc, 2)  # PW_RENDERFULLCONTENT
$g.ReleaseHdc($hdc)

$nonBlack = 0
if ($ok) {
    for ($y = 0; $y -lt $h; $y += 7) {
        for ($x = 0; $x -lt $w; $x += 7) {
            $p = $bmp.GetPixel($x, $y)
            if ($p.R + $p.G + $p.B -gt 24) { $nonBlack++ }
        }
    }
}
if (-not $ok -or $nonBlack -lt 40) {
    $g.Dispose(); $bmp.Dispose()
    Write-Host "PrintWindow insufficient (ok=$ok, lit=$nonBlack); falling back to screen grab"
    $bmp = New-Object System.Drawing.Bitmap($w, $h, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($rect.Left, $rect.Top, 0, 0, (New-Object System.Drawing.Size($w, $h)))
    $g.Dispose()
} else {
    Write-Host "PrintWindow ok, lit samples=$nonBlack"
    $g.Dispose()
}

$dir = Split-Path -Parent $OutFile
if ($dir) {
    if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    $path = Join-Path (Resolve-Path -LiteralPath $dir).Path (Split-Path -Leaf $OutFile)
} else {
    $path = Join-Path (Get-Location).Path $OutFile
}
$bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Host "saved $path ($w x $h)"
