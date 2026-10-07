# Windows 安装

## 版权与素材声明（先读这一段）

- 本程序是《明日方舟：终末地》的**同人二创工具**，与鹰角网络 / Mountain Contour /
  悠星无隶属关系，也不是官方软件。
- **源码仓库里不含任何音频、图片、字体或安装包**；界面配色取自游戏设置页，
  版权属于权利人。
- 发行包（`sounds/`、`background.jpg`）里的语音与背景图同样属于权利人，
  只作为**个人学习、非商业**用途随包分发。把它们用于任何商业用途、二次分发
  或对外传播都可能构成侵权，请自行判断并承担后果。
- 权利人如认为侵权，开 issue 或发邮件即可，我们会立刻从发行渠道下架。
- 你随时可以换掉它们：设置 → 播报事件 → 替换 / 恢复，或设置 → 外观 → 背景图片；
  换掉之后本程序不保留任何副本，也不会把你提供的文件上传到任何地方。

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

## 托盘常驻

装好服务后，登录时服务除了代理还会拉起一个常驻托盘的设置窗口
（启动页 → 托盘常驻，出厂开着）：

- 左键点托盘图标，或右键菜单里的「打开设置」= 唤回窗口；
- 窗口右上角的 × 只是**收进托盘**，不退出；要真退出去菜单里点「退出」；
- 桌面没有通知区域时（少数精简系统），× 退回成"直接退出"。

Windows 11 会把新出现的托盘图标收进任务栏的 `^` 溢出面板，第一次要手动
把它拖到外边才算常驻可见。不想常驻就把「托盘常驻」关掉，下次登录起生效。

查看状态 / 卸载：

```powershell
sc.exe query edscene-broadcast
edscene --uninstall-service
```

（PowerShell 里必须写 `sc.exe`：裸 `sc` 是 `Set-Content` 的别名。）

## 换了新版 exe 之后

服务进程会锁住旧 exe，直接覆盖会报"拒绝访问"。在**管理员**窗口里按顺序：

```powershell
sc.exe config edscene-broadcast start= disabled   # 先断掉自动重启
taskkill /F /IM edscene.exe                        # 结束服务与代理进程
Copy-Item D:\works\edscene\target\release\edscene.exe 'C:\Program Files\edscene\edscene.exe' -Force
& 'C:\Program Files\edscene\edscene.exe' --install-service   # 会把启动类型改回自动
```

`--install-service` 自带停-再启，不需要手工 `net start`。

不想装服务也能自启：任务管理器 → 启动 → 新建一条指向
`edscene.exe --agent` 的启动项（轻量模式，代价是登录界面上不出声）。
设置窗口里的"开机自启"开关改的就是服务的启动类型，不会再往
`HKCU\...\Run` 里塞东西，避免两套自启打架。

## 文件位置

没装服务时一切都在用户目录；装了服务（`%PROGRAMDATA%\edscene\` 存在）之后，
服务、界面和每个会话里的代理共用机器级那一份：

```
%APPDATA%\edscene\config.toml        用户级设置（第一次装服务时被复制成下面那份）
%PROGRAMDATA%\edscene\config.toml    装了服务之后，服务与界面共用这一份
%PROGRAMDATA%\edscene\sounds\        你"替换"过的音频
%PROGRAMDATA%\edscene\sounds\.disabled\  退回预设时被移走的旧替换文件（不会删）
%PROGRAMDATA%\edscene\state\events.ndjson 服务日志：事件判定、下发、代理上线
%LOCALAPPDATA%\edscene\events.ndjson  你自己这一份：界面与代理的播放记录
```

两份日志分开是必须的：机器级那份由服务（LocalSystem）创建，普通用户追加不进去；
代理是以你的身份跑的，所以它的 `play` 记录只会出现在用户级那份里。排查一次播报
要两边都看：`fire` 在服务这份，`play` 在你这份。

`edscene --config-path` 打印实际在用的路径。第一次以用户身份保存设置时会把
用户级配置复制进 `%PROGRAMDATA%`（由谁创建就归谁，所以界面之后还能改），
服务改完设置约两秒内自动生效，不用重启。

## 排查

| 现象 | 怎么办 |
|---|---|
| 完全不出声 | `edscene --selftest` 看状态列；`未找到文件` 就是素材目录不对 |
| 显示 `格式不支持` | 该文件是 Opus/WMA，用 ffmpeg 转成 `.wav` 或 `.flac` 再放回素材目录 |
| 只播一次就不播了 | 看 `events.ndjson` 里有没有 `skipped / busy`；`--simulate` 会告诉你输出流有没有被释放 |
| 锁屏没声音 | 出厂默认"锁屏"是关的：`edscene --enable lock` |
| 服务起不来 | `sc.exe query edscene-broadcast`，再看 `%PROGRAMDATA%\edscene\state\events.ndjson` |
| 装完没声音但服务显示 RUNNING | 老版本假死；按上面"换了新版 exe 之后"重启一次。新版会在安装结束时确认服务还活着，并在失败时打印 `os error` 编号 |
| 想彻底重来 | `edscene --factory-reset`（只动配置和替换文件，不碰你的素材目录） |

## 关于"登录界面就出声"

默认关闭。打开它（设置 → 启动 → 登录界面播报）会让服务复制 `winlogon.exe`
的令牌，在还没人登录的控制台会话里拉一个播报进程 —— 这是唯一能在密码输入框
前出声的办法，但它需要 `SE_ASSIGNPRIMARYTOKEN_NAME` 特权，且在快速用户切换、
远程桌面下行为不稳定。默认启用的五个事件全都在登录之后，所以正常用不到它。
