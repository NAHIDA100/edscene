# Linux 安装

## 装包

```sh
sudo dpkg -i edscene_0.1.0_amd64.deb && sudo apt -f install
# 或者解压 tar.gz：
sudo install -m755 edscene /usr/local/bin/edscene
```

依赖只有桌面本来就有的东西：ALSA/PipeWire 或 PulseAudio 的输出库
（`libasound2`），以及 D-Bus。音频解码全部静态编译进二进制，
不需要 ffmpeg、gstreamer 或任何外部播放器。

## 第一次跑

```sh
edscene --selftest --sounds ~/声音包
edscene --agent &          # 前台试一轮，Ctrl+C 结束
```

`--agent` 会读 login1 / UPower / 屏保状态。想确认它到底看到了什么：

```sh
edscene --watch
# [      0 ms] Power { on_ac: true, percent: 96, saver: None }
# [    401 ms] Power { ... }
# [  12844 ms] Unlock { session: 2 }
#             ↳ 播报 解锁返回桌面
```

## 开机自启（不需要 root）

设置窗口 → 启动 → 开机自启，或者：

```sh
mkdir -p ~/.config/systemd/user ~/.config/autostart
# 由程序写入，等价于：
edscene --agent   # 手动确认没问题后再开自启
systemctl --user --now enable edscene.service
loginctl enable-linger "$USER"     # 可选：登录后更早启动
```

写出来的两个文件：

- `~/.config/systemd/user/edscene.service` —— `PartOf=graphical-session.target`、
  `Restart=on-failure`、`Nice=10`，保证它在图形会话里、优先级低于桌面本身；
- `~/.config/autostart/io.github.edscene.desktop` —— 给没有 systemd 用户实例的
  桌面（纯 WM、部分 BSD 风格环境）兜底。

关掉自启会同时删掉这两个文件。

## 想更早：登录界面（greeter）阶段

GDM 的 greeter 跑在 `Class=greeter` 的会话里，用的是 `gdm` 用户自己的
PipeWire 套接字。要让播报在输密码之前就响，需要：

1. 一个系统级单元（需要 root）：

   ```sh
   sudo install -m644 packaging/systemd/edscene-system.service /etc/systemd/system/
   sudo systemctl daemon-reload && sudo systemctl --now enable edscene-system
   ```

2. 该服务以 `gdm` 身份访问其运行时目录里的音频套接字
   （`XDG_RUNTIME_DIR=/run/user/<gdm-uid>`）。**这一步在不同发行版上差别很大**：
   systemd-homed、非标准 `User=` 沙箱、以及 hardened 的 PipeWire 配置都可能拒绝它。

因此本项目的默认路径是"用户级、不需要 root"，系统级单元作为可选项，
并且文档不承诺它在所有发行版上工作。已验证逻辑与发行版差异是两回事，
这里分开说清楚。

## 各桌面的差异

| 桌面 | 锁/解锁 | 屏保 | 电量 | 托盘 |
|---|---|---|---|---|
| GNOME (Wayland) | login1 `LockedHint` ✓ | `org.gnome.ScreenSaver` ✓ | UPower ✓ | 需要 AppIndicator 扩展 |
| KDE Plasma | login1 ✓ | `org.freedesktop.ScreenSaver` ✓ | UPower ✓ | 正常 |
| 混合 WM + lightdm | login1 ✓ | 通常没有会话总线屏保服务 → 屏保事件静默跳过 | UPower ✓ | 取决于 statusnotifier |

托盘不可用时，窗口照样能用 `edscene`（不带参数）打开，或再启一次 `--agent`。

## 从源码构建

```sh
sudo apt-get install -y libasound2-dev libdbus-1-dev pkg-config
cargo build --release          # 产物 target/release/edscene
cargo test --workspace         # 含 eds-linux 的 desktop 单测
./packaging/build-deb.sh target/release/edscene edscene_local_amd64.deb
```

## 已知限制（诚实清单）

- 屏保事件依赖会话总线上的 `ScreenSaver` 服务；没有它就没有"屏保恢复"。
- "低电量模式"在 Linux 上是从 UPower 的 `OnBattery` + 百分比**派生**的，
  不是桌面环境真正的省电模式状态。
- 壁纸模糊背景优先读 `gsettings` 的壁纸路径，其次试 `grim`/`import`/`scrot`；
  都没有时自动退回纯色，不报错。
- 本项目的 Linux 二进制由 CI 构建并跑测试；作者的开发机是 Windows，
  所以发行版专属行为（尤其 greeter 出声）以 issue 报告为准。
