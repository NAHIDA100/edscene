# edscene · 集成工业播报终端

明日方舟：终末地 的**同人二创**桌面工具。它盯着系统的登录、解锁、锁屏、屏保、
电源与电量变化，在事件发生的那一刻播报你指定的语音 —— 像基地播报，也像系统提示音。

界面照搬游戏里"设置/外观"那一套：谷地黄 `#F5FF01` 强调色、半透明行带、45° 斜纹、
白色胶囊控件、`//设置` 页眉、右下角"恢复默认"。

> **不含任何官方素材。** 仓库里没有音频、没有图片、没有字体、没有安装包；
> 程序在你指向自己的素材文件夹之前不会出声。
> 本项目与 鹰角网络 / Mountain Contour / 悠星 无任何隶属或授权关系，仅供个人学习使用，
> 不得用于商业用途。侵权请联系 issue，会直接下架。

---

## 30 秒跑起来

### Windows

```powershell
# 1) 解压 edscene-<版本>-windows-x64.zip，把 edscene.exe 放到任意目录
# 2) 指向你的播报音频文件夹（文件名就是事件名）
edscene --selftest --sounds "D:\works\eds"
# 3) 装成开机自启的服务（弹一次 UAC）
edscene --install-service
# 4) 打开设置窗口
edscene
```

第 2 步会打印 8 个事件各自找到了哪个文件；第 3 步之后，**开机在你输密码之前服务就已经起来了**。

### Linux

```sh
sudo dpkg -i edscene_0.1.0_amd64.deb     # 或者解压 tar.gz 自己放 /usr/local/bin
edscene --selftest --sounds ~/声音包
edscene --agent &                        # 先手动试一轮
```

设置窗口里把"开机自启"打开，它会写 `~/.config/systemd/user/edscene.service`
（外加一份 XDG autostart 兜底，非 systemd 的桌面也能用）。详见
[docs/install-linux.md](docs/install-linux.md)。

## 素材怎么放

八个事件的**出厂预设就是文件名**，按主干匹配、不看扩展名：

| 事件 | 预设文件名 | 出厂开关 |
|---|---|---|
| 解锁返回桌面 | `解锁返回桌面.mp3` | **开** |
| 断开电源（电源适配器） | `断开电源（电源适配器）.mp3` | **开** |
| 电源恢复接入 | `电源恢复接入.mp3` | **开** |
| 低电量模式 | `低电量模式.mp3` | **开** |
| 电池电量低于5% | `电池电量低于5%.mp3` | **开** |
| 开机进入桌面 | `开机进入桌面.mp3` | 关 |
| 锁屏 | `锁屏.mp3` | 关 |
| 屏保恢复 | `屏保恢复.mp3` | 关 |

- 文件名里的全角括号和 `%` 都按字面匹配，别改。
- **扩展名可以随便换**：`锁屏.wav`、`unlock.m4a`、`锁屏.flac` 都能被认出来 ——
  真实格式是嗅探文件头得出的，不是看后缀（顺带说一句：常见的游戏语音导出包
  后缀写着 `.mp3`，里面其实是 AAC-in-MP4）。
- 支持 MP3 / M4A(MP4-AAC) / ADTS-AAC / WAV / FLAC / Ogg Vorbis / AIFF。
  **不支持 Opus 和 WMA**，遇到会明确报"格式不支持"，不会闷声不响。

## 换掉某个声音，再恢复出厂

三层优先级，从高到低：

1. **替换目录** `%APPDATA%\edscene\sounds\<事件id>.*`（Linux：`~/.config/edscene/sounds/`）
   —— 界面上点"替换"就是写这里。
2. **配置里给这一行单独指定的绝对路径**。
3. **素材目录里的预设文件名** —— 直接把你文件夹里的 `锁屏.mp3` 换成别的，
   下次播放就生效，不用重启，也不用改任何设置。

- 单行"恢复"：只把这一个事件退回预设，替换过的文件**移动**到 `sounds\.disabled\`，不删。
- 右下角"恢复默认"（点两次确认）：所有设置回到出厂，全部替换文件退回 `.disabled`。
- 命令行等价物：`edscene --factory-reset`。

## 事件是怎么判定的

规则全部在 `eds-core` 里，是一个**不碰操作系统**的纯状态机（45 个单元测试覆盖）。
平台侧只负责把原始信号喂进来：

| 事件 | Windows | Linux |
|---|---|---|
| 开机进入桌面 | 服务启动 90 秒内、控制台会话第一次 `WTS_SESSION_LOGON` | seat0 上 `Class` 从 greeter 变 user 且是本次开机第一次 |
| 解锁返回桌面 | `WTS_SESSION_UNLOCK` / `CONSOLE_CONNECT` | login1 `LockedHint` false→true |
| 锁屏 | `WTS_SESSION_LOCK` | login1 `LockedHint` true→false |
| 屏保恢复 | `GUID_CONSOLE_DISPLAY_STATE` 变亮且期间没锁屏 | 会话总线 `ScreenSaver.GetActive()` 变 false |
| 低电量模式 | 轮询 `GetSystemPowerStatus`：离线且电量 ≤ 阈值（默认 20%，近似值） | UPower `OnBattery` + 百分比 |
| 电池电量低于5% | 下降沿触发，回到 8% 才重新武装 | UPower `DisplayDevice.Percentage`，同一套迟滞 |
| 断开 / 接入电源 | `ACLineStatus` 边沿 | UPower `OnBattery` 边沿 |

细节见 [docs/events.md](docs/events.md)。

## 为什么开机就得起服务

服务（LocalSystem，自动启动）在**输密码之前**就加载完毕，它是唯一的事件源；
真正出声的是它在你登录瞬间拉进会话的代理进程 —— 代理里没有一行监听代码，
所以结构上不可能重复播报。锁屏/解锁只是切桌面不换会话，代理常驻即可，
所以默认启用的五个事件全都不需要"登录界面出声"。

想让播报在**密码界面**上就响（比如开机进桌面那条），在设置里打开
"登录界面播报"（复制 winlogon 令牌的实验路径，默认关闭）。

## 命令行

```
edscene                      打开设置窗口
edscene --selftest [事件]     检查 8 个预设是否就绪；带事件名时顺便试听
edscene --simulate a,b        不等系统事件，按顺序播报（走完整链路）
edscene --signals f.jsonl     把一份信号脚本喂进状态机，验证判定逻辑
edscene --watch               打印原始信号与播报决定，不出声
edscene --enable/--disable X  脚本化开关某个事件
edscene --factory-reset       恢复出厂
edscene --config-path         配置 / 素材 / 日志位置
edscene --install-service     Windows：注册并启动开机自启服务（需管理员）
edscene --uninstall-service   Windows：停止并移除服务
```

## 占用

- 单个 `edscene.exe`，无 DLL、无运行时、无安装器。
- 代理进程常驻约 10 MB 量级；**不播报时不持有音频设备**（输出流在最后一次
  播放 5 秒后释放），服务进程不碰音频也不开窗口。
- 电源状态是 2 秒一次的轮询（一次 `GetSystemPowerStatus`），Linux 侧是 400 ms
  一次的本地 socket 属性读取 —— 换来的是不需要异步运行时。

## 从源码构建

```sh
cargo build --release            # Windows
cargo test --workspace           # 45 个纯逻辑测试，不碰操作系统
```

中国网络下需要配 crates.io 镜像，见 [docs/dev-setup-windows.md](docs/dev-setup-windows.md)。

## 许可

- 代码：MIT（`LICENSE-code`）
- 皮肤、文案、预设名称：CC BY-NC-ND 4.0（`LICENSE-content`）
- 免责声明与第三方组件清单：`NOTICE`

## 致谢

界面配色取自《明日方舟：终末地》的设置页；游戏本体与其素材的版权属于
鹰角网络 / Mountain Contour / 悠星。这个项目只是想把基地播报搬到桌面上。
