# edscene 安装脚本（Windows）
#
# 双击同目录的 setup.bat 即可；它会自己申请管理员权限。
# 也可以直接跑：powershell -File install.ps1 -Target "D:\edscene"
#
# 它做四件事：确认版权声明 -> 复制程序与素材 -> 注册开机自启服务 ->
# 在开始菜单建快捷方式。卸载用「设置 → 应用」或 edscene --uninstall-service。

param(
    [string]$Target = "$env:ProgramFiles\edscene",
    [switch]$Yes
)

$ErrorActionPreference = 'Stop'
# 这个脚本和 edscene.exe 放在同一层，发行包解开后直接双击 setup.bat 即可。
$src = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = $src

# ---- 1. 提权：没管理员就重新以管理员启动自己
$me = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $me.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    "需要管理员权限（注册服务），正在弹出 UAC……"
    Start-Process powershell -Verb RunAs -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", '-Target', "`"$Target`"", '-Yes'
    )
    exit $LASTEXITCODE
}

# ---- 2. 版权声明：装之前必须看清楚
$note = @'
本程序是《明日方舟：终末地》的同人二创工具，与鹰角网络 / Mountain Contour / 悠星
无隶属关系，也不是官方软件。

随包分发的 sounds\ 语音与 background.jpg 背景图，版权归权利人所有，仅供个人学习、
非商业用途。用于商业、二次分发或对外传播都可能构成侵权，请自行判断并承担后果。
权利人如认为侵权，开 issue 或发邮件即可，我们会立刻从发行渠道下架。

装好后你随时可以换掉这些素材（设置 → 播报事件 → 替换 / 恢复，设置 → 外观 → 背景
图片），本程序不保留副本，也不会把任何文件上传到网络。
'@
Write-Host ''
Write-Host $note
Write-Host ''
if (-not $Yes) {
    $ans = Read-Host '接受以上声明并继续安装？输入 yes 继续，其它任意键取消'
    if ($ans -ne 'yes') { '已取消。'; exit 1 }
}

# ---- 3. 复制
New-Item -ItemType Directory -Force -Path $Target | Out-Null
Copy-Item (Join-Path $root 'edscene.exe') $Target -Force
foreach ($f in 'README.md', 'CHANGELOG.md', 'LICENSE-code', 'LICENSE-content', 'NOTICE', 'background.jpg') {
    $p = Join-Path $root $f
    if (Test-Path $p) { Copy-Item $p $Target -Force }
}
if (Test-Path (Join-Path $root 'docs')) {
    Copy-Item (Join-Path $root 'docs') $Target -Recurse -Force
}
if (Test-Path (Join-Path $root 'sounds')) {
    New-Item -ItemType Directory -Force -Path (Join-Path $Target 'sounds') | Out-Null
    Copy-Item (Join-Path $root 'sounds\*') (Join-Path $Target 'sounds') -Force
}
"已安装到 $Target"

# ---- 4. 服务：开机自启 + 输密码之前就加载
& (Join-Path $Target 'edscene.exe') --install-service
if ($LASTEXITCODE -ne 0) { throw "服务注册失败，见上面的错误号" }

# ---- 5. 开始菜单快捷方式（打设置窗口用）
$lnk = Join-Path ([Environment]::GetFolderPath('Programs')) '集成工业播报终端.lnk'
$sh = New-Object -ComObject WScript.Shell
$sc = $sh.CreateShortcut($lnk)
$sc.TargetPath = Join-Path $Target 'edscene.exe'
$sc.WorkingDirectory = $Target
$sc.Description = '集成工业播报终端（同人二创）'
$sc.Save()
"快捷方式：$lnk"

Write-Host ''
'装好了。锁屏再解锁一次就会听到「解锁返回桌面」。'
'不想出声的开关在：设置 → 播报事件。'
