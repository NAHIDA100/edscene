# Windows 安装

## 直接跑（不装服务）

1. 解压 `edscene-<版本>-windows-x64.zip`，把 `edscene.exe` 放到任意目录
   （建议 `C:\Program Files\edscene\`，避免和"下载"一起被清掉）。
2. 双击运行 = 打开设置窗口。第一次先选素材目录，或用命令行：

   ```powershell
   edscene --selftest --sounds "D:\works\eds"
   ```

   看到 `就绪 5/5 个已启用的播报。` 就说明八个文件都认出来了。
3. 不出声的话先单独试一条：

   ```powershell
   edscene --selftest unlock
   edscene --simulate unlock,ac_lost
   ```

## 开机自启（推荐，需要管理员）

```powershell
edscene --install-service
```

会弹一次 UAC。它做的事：

- 注册服务 `edscene-broadcast`，显示名"集成工业播报终端"，LocalSystem，自动启动；
- 崩溃后 5s / 30s / 60s 自动重启；
- 立刻启动一次。

之后每次开机：服务在**登录界面出现之前**就已加载；你一输完密码，
服务用你的会话令牌把代理进程拉进桌面（`winsta0\Default`），
所以"输完密码进桌面"这一声不会漏。锁屏/解锁只是切桌面，代理常驻，照常播报。

查看状态 / 卸载：

```powershell
sc query edscene-broadcast
edscene --uninstall-service
```

不想装服务也能自启：任务管理器 → 启动 → 新建一条指向
`edscene.exe --agent` 的启动项（轻量模式，代价是登录界面上不出声）。
设置窗口里的"开机自启"开关改的就是服务的启动类型，不会再往
`HKCU\...\Run` 里塞东西，避免两套自启打架。

## 文件位置

```
%APPDATA%\edscene\config.toml        设置
%PROGRAMDATA%\edscene\config.toml    装了服务之后，服务与界面共用这一份
%APPDATA%\edscene\sounds\            你"替换"过的音频
%APPDATA%\edscene\sounds\.disabled\  退回预设时被移走的旧替换文件（不会删）
%LOCALAPPDATA%\edscene\events.ndjson 事件日志（原始信号 + 播报决定）
```

`edscene --config-path` 会打印实际在用的路径。装了服务时以
`%PROGRAMDATA%` 那份为准，因为服务读不到你的用户目录也正常。

## 排查

| 现象 | 怎么办 |
|---|---|
| 完全不出声 | `edscene --selftest` 看状态列；`未找到文件` 就是素材目录不对 |
| 显示 `格式不支持` | 该文件是 Opus/WMA，用 ffmpeg 转成 `.wav` 或 `.flac` 再放回素材目录 |
| 只播一次就不播了 | 看 `events.ndjson` 里有没有 `skipped / busy`；`--simulate` 会告诉你输出流有没有被释放 |
| 锁屏没声音 | 出厂默认"锁屏"是关的：`edscene --enable lock` |
| 服务起不来 | `sc query edscene-broadcast`，再看 `%PROGRAMDATA%\edscene\state\events.ndjson` |
| 想彻底重来 | `edscene --factory-reset`（只动配置和替换文件，不碰你的素材目录） |

## 关于"登录界面就出声"

默认关闭。打开它（设置 → 启动 → 登录界面播报）会让服务复制 `winlogon.exe`
的令牌，在还没人登录的控制台会话里拉一个播报进程 —— 这是唯一能在密码输入框
前出声的办法，但它需要 `SE_ASSIGNPRIMARYTOKEN_NAME` 特权，且在快速用户切换、
远程桌面下行为不稳定。默认启用的五个事件全都在登录之后，所以正常用不到它。
