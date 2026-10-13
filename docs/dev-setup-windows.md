# 开发环境（Windows）

## 需要装什么

```powershell
winget install -e --id Rustlang.Rustup
winget install -e --id BrechtSanders.WinLibs.POSIX.MSVCRT   # 提供 dlltool/as/ar
```

不需要 Visual Studio。`x86_64-pc-windows-gnu` 工具链自带链接器，
但 `windows` crate 生成导入库时要调 `dlltool.exe`，而那个 dlltool 又要调
同目录下的 `as.exe` / `ar.exe` —— rustup 自带的那份没有这些兄弟进程，
所以必须指向一个完整的 MinGW。

## 中国网络下的两处坑

`static.crates.io` 会被 TLS 直接重置（`curl` 返回 000），而稀疏索引
`index.crates.io` 是通的。所以必须换镜像，否则 `cargo build` 会在下载阶段卡死，
看起来像"依赖树太大编译慢"。

写进 `%USERPROFILE%\.cargo\config.toml`（**这台机器的配置，不要提交进仓库**）：

```toml
[source.crates-io]
replace-with = "rsproxy"

[source.rsproxy]
registry = "sparse+https://rsproxy.cn/index/"

[registries.rsproxy]
index = "sparse+https://rsproxy.cn/index/"

[net]
git-fetch-with-cli = true

[target.x86_64-pc-windows-gnu]
linker = "C:\\Users\\<你>\\AppData\\Local\\Microsoft\\WinGet\\Packages\\BrechtSanders.WinLibs.POSIX.MSVCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\\mingw64\\bin\\x86_64-w64-mingw32-gcc.exe"
rustflags = ["-C", "dlltool=C:\\Users\\<你>\\AppData\\Local\\Microsoft\\WinGet\\Packages\\BrechtSanders.WinLibs.POSIX.MSVCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\\mingw64\\bin\\dlltool.exe"]
```

两个绝对路径是故意的：winget 给 WinLibs 注册的是"应用执行别名"，
通过别名启动的 `gcc` 会算错自己的安装前缀，于是找不到
`x86_64-w64-mingw32/lib`，报一个完全看不出原因的
`ld: cannot find -lshlwapi`。写死真实路径就没事。

`rustup-init.exe` 也要用完整描述名，`--default-toolchain x86_64-pc-windows-gnu`
会被判成非法名字：

```powershell
rustup-init.exe -y --default-toolchain stable-x86_64-pc-windows-gnu --profile minimal
```

## 常用命令

```powershell
cargo test --workspace                              # 纯逻辑测试，不碰操作系统
cargo run -- --selftest --sounds "D:\works\eds"
cargo run -- --watch                                # 只看信号，不出声
cargo build --release --workspace
```

服务相关的命令需要管理员，建议直接开一个提升过的终端跑
`target\release\edscene.exe --install-service`。

## 出安装程序（中文向导 setup.exe）

```powershell
cargo build --release
powershell -NoProfile -ExecutionPolicy Bypass -File packaging\windows\pack.ps1 `
  -Assets "D:\works\eds" -Background "C:\path	o\picture.jpg" -Out "D:\works\edscene-dist"
```

`pack.ps1` 把暂存树交给 Inno Setup（`packaging\windows\setup.iss`）编译，产物是
`edscene-setup-<版本>-windows-x64.exe`。素材只在编译时进安装包，仓库里永远没有。
需要本机装有 Inno Setup 6（默认找 PATH 里的 ISCC.exe，或用 `-Iscc` 指定路径）。

停旧服务的逻辑在 `packaging\windows\service-control.ps1`：它只认"服务镜像路径等于
本次安装目录"的那一份，按可执行文件路径逐个结束进程，不 `taskkill /IM` 全杀，也不会
误伤构建目录里跑着的 exe。

## 交叉检查 Linux 代码

应用层（拉了 cpal/alsa）没法在 Windows 上交叉编译，但 `eds-linux`
是纯 Rust，可以只做类型检查 —— 这是唯一能在改 D-Bus 代码后立刻拿到反馈的方式：

```powershell
rustup target add x86_64-unknown-linux-gnu
cargo check -p eds-linux --target x86_64-unknown-linux-gnu --all-targets
```

`cargo test --target x86_64-unknown-linux-gnu` 会失败，因为链接需要 Linux 的
`cc`；测试交给 CI 的 ubuntu runner。

## 界面验证（不开浏览器）

```powershell
start target\release\edscene.exe
powershell -NoProfile -File scripts\shot.ps1 -ProcessName edscene -OutFile shot.png
```

`shot.ps1` 用 `PrintWindow` 抓窗口（被遮挡也能抓）。它开头会调
`SetProcessDPIAware()` —— 少了这一句，在 125% / 150% 缩放的机器上
`GetWindowRect` 返回的是虚拟化坐标，你会拿到一张只包含窗口左上角、
右边和下边凭空消失的截图，而它看起来完全像"布局溢出"的 bug。

## 提交守卫

```powershell
git config core.hooksPath .githooks
python scripts\check_repo_hygiene.py --all
```

任何 `.mp3/.m4a/.wav/...`、任何安装包/可执行文件/字体、以及超过 200 KB 的文件
都会被拒绝；未登记在 `ALLOWED_IMAGES` 里的图片也会被拒绝（防止顺手提交一张
游戏截图）。CI 里再跑一遍，`--no-verify` 绕不过去。

## 依赖版本备忘

| 组件 | 版本 | 注意 |
|---|---|---|
| slint | 1.18.1 | 1.18 起 feature 名改成 `std` / `backend-winit` / `renderer-software` / `software-renderer-systemfonts`；中文字形要靠 `software-renderer-systemfonts`，否则软渲染用内嵌字体集，CJK 会变方框 |
| rodio | 0.22.2 | **没有** `playback` 之外的 `default`；API 是 `DeviceSinkBuilder::open_default_sink()` + `Player::connect_new(mixer)`，不是老教程里的 `OutputStream::try_default()`；`Source` 是 `Iterator<Item=f32>`，没有 `read()` |
| rodio `mp4` feature | 必需 | 它 = symphonia 的 `isomp4` + `aac`。只开 `mp3` 解不了那些后缀写着 `.mp3`、实际是 M4A 的语音包 |
| zbus | 5.x | 需要 `async-io`（默认 feature）+ `blocking-api`；只关 `default-features` 会编译期报错 |
| windows | 0.62 | WTS 函数在 `Win32::System::RemoteDesktop`（不是 TerminalServices），GUID 在 `System::SystemServices`，`CreateEnvironmentBlock` 在 `System::Environment` |
| windows-service | 0.8.1 | `ServiceStartType::AutoStart`，`ServiceControlHandlerResult` 在 `service_control_handler` 模块里 |
| eds-setup 用的 windows API | 0.62 | `GetTokenInformation` / `TOKEN_ELEVATION` 在 `Win32::Security`（不是 `System::Security`）；crate 里没有导出 `CLSID_ShellLink`，要自己用 `GUID::from_u128(0x00021401-…-000000000046)`；`ShellExecuteW` 返回 `HINSTANCE`，值 ≤32 才是失败（1223 = 用户点了"否"）|
