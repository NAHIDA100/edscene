param(
    [Parameter(Mandatory = $true)][ValidateSet('Stop', 'Restore')][string]$Mode,
    [Parameter(Mandatory = $true)][string]$InstallDir,
    [string]$StateFile
)

# 只处理"安装目录里那一份"服务：先核对服务镜像路径，再动手。绝不按进程名全杀，
# 否则开发者机器上构建目录里的 edscene.exe 会被连带干掉。
$ErrorActionPreference = 'Stop'
$name = 'edscene-broadcast'
$target = [IO.Path]::GetFullPath((Join-Path $InstallDir 'edscene.exe'))

function ImagePath {
    $svc = Get-CimInstance -ClassName Win32_Service -Filter "Name='$name'" -ErrorAction SilentlyContinue
    if (-not $svc -or -not $svc.PathName) { return $null }
    $p = $svc.PathName.Trim()
    if ($p.StartsWith('"')) { $p = ($p -split '"')[1] } else { $p = ($p -split ' ')[0] }
    try { return [IO.Path]::GetFullPath($p) } catch { return $null }
}

function LocksFile([string]$path) {
    try {
        $fs = [IO.File]::Open($path, 'Open', 'ReadWrite', 'None')
        $fs.Close()
        return $false
    } catch [System.IO.IOException] {
        return $true
    } catch {
        return $false
    }
}

function State([bool]$running, [string]$startMode) {
    if (-not $StateFile) { return }
    (@{ running = $running; startMode = $startMode } | ConvertTo-Json -Compress) |
        Set-Content -LiteralPath $StateFile -Encoding ASCII
}

if ($Mode -eq 'Restore') {
    if (-not $StateFile -or -not (Test-Path -LiteralPath $StateFile)) { exit 0 }
    $saved = Get-Content -LiteralPath $StateFile -Raw | ConvertFrom-Json
    $svc = Get-CimInstance -ClassName Win32_Service -Filter "Name='$name'" -ErrorAction SilentlyContinue
    if (-not $svc) { exit 0 }
    if (ImagePath) {
        Set-Service -Name $name -StartupType $saved.startMode -ErrorAction SilentlyContinue
        if ($saved.running) { Start-Service -Name $name -ErrorAction SilentlyContinue }
    }
    exit 0
}

$svc = Get-CimInstance -ClassName Win32_Service -Filter "Name='$name'" -ErrorAction SilentlyContinue
if (-not $svc) { exit 0 }
$image = ImagePath
if (-not $image) {
    Write-Error "无法解析服务 $name 的镜像路径，放弃（不动它）。"
    exit 2
}
if ($image.ToLowerInvariant() -ne $target.ToLowerInvariant()) {
    Write-Error "服务指向 $image，不是本次要覆盖的 $target；请先卸掉那一份再装。"
    exit 2
}
$wasRunning = $svc.State -eq 'Running'
$wasStart = switch ($svc.StartMode) {
    'Auto' { 'Automatic' }
    'Manual' { 'Manual' }
    'Disabled' { 'Disabled' }
    default { 'Manual' }
}
State $wasRunning $wasStart

# 断掉"失败自动重启"，否则杀掉进程后 SCM 会立刻把它拉回来，锁着文件不放。
& sc.exe config $name start= disabled | Out-Null

if ($wasRunning) {
    try {
        Add-Type -AssemblyName System.ServiceProcess
        $c = New-Object System.ServiceProcess.ServiceController $name
        $c.Stop()
        $c.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(8))
        $c.Dispose()
    } catch {
        Write-Host "停止请求超时，改为逐个结束该镜像的进程。"
    }
}

Get-CimInstance Win32_Process -Filter "Name='edscene.exe'" | Where-Object {
    $_.ExecutablePath -and $_.ExecutablePath.ToLowerInvariant() -eq $target.ToLowerInvariant()
} | ForEach-Object {
    Write-Host "结束残留进程 PID $($_.ProcessId)"
    Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
}

for ($i = 0; $i -lt 25; $i++) {
    if (-not (LocksFile $target)) { exit 0 }
    Start-Sleep -Milliseconds 200
}

# 文件仍被占用：把启动类型恢复原样，别把系统留在 disabled 状态。
& sc.exe config $name start= $wasStart | Out-Null
if ($wasRunning) { Start-Service -Name $name -ErrorAction SilentlyContinue }
Write-Error "$target 仍被占用，安装中止（服务与启动类型已恢复原状）。"
exit 3
