# 隐私与数据

## 程序读写什么

| 数据 | 位置 | 内容 |
|---|---|---|
| 配置 | `%APPDATA%\edscene\config.toml`（装了服务则 `%PROGRAMDATA%\edscene\config.toml`） | 素材目录路径、每个事件的开关与音量、界面偏好 |
| 替换素材 | 同目录 `sounds\` | 你自己挑的音频副本 |
| 事件日志 | `%LOCALAPPDATA%\edscene\events.ndjson` | 时间戳 + 事件类型 + 播放了哪个文件，超过 1 MB 轮转一次 |
| 自启注册 | Windows 服务 `edscene-broadcast` / Linux 用户单元 | 只有可执行文件路径和启动参数 |

## 不做什么

- **没有任何网络访问**。代码里没有 HTTP 客户端、没有遥测、没有更新检查，
  依赖树里也没有会连网的组件（`slint`/`rodio`/`zbus` 都不联网）。
  防火墙规则不需要为它开任何口子。
- **不上传、不记录内容**。日志里只有"哪个事件、什么时刻、播了哪个文件名"，
  不包含音频内容、不包含按键、不包含剪贴板、不包含窗口标题。
- **不读取音频目录以外的文件**。素材目录由你显式指定，
  并且默认拒绝网络共享路径（`sounds.allow_network_share` 才放开）。
- **不注入任何进程**。Windows 侧只有服务在自己的会话里启动一个自己的子进程；
  不加载 DLL 到别人的进程，不装驱动，不做键盘钩子。

## 服务为什么需要 LocalSystem

不是为了权限本身，而是因为"在你输完密码之前就得在"这一条要求：
只有开机即启动的系统服务能早于登录界面就绪。它做的全部事情是：

1. 读会话变化通知（登录/锁屏/解锁）与系统电源状态；
2. 在你登录的那一刻，用**你自己的**会话令牌启动播报代理；
3. 通过命名管道告诉代理"播第 N 号事件"。

它不读你的密码、不参与认证、不监听任何网络端口。命名管道
`\\.\pipe\edscene.agent.v1` 只有本机会话内的进程能连，且首帧必须是有版本号的握手。

## 卸载

```powershell
edscene --uninstall-service      # 删服务
Remove-Item "$env:APPDATA\edscene", "$env:LOCALAPPDATA\edscene" -Recurse
```

Linux：

```sh
systemctl --user --now disable edscene.service
rm ~/.config/systemd/user/edscene.service ~/.config/autostart/io.github.edscene.desktop
rm -rf ~/.config/edscene ~/.local/state/edscene
```
