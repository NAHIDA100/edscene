# 事件判定细节

规则只有一份，在 `eds-core::dispatcher`。平台代码只产出 `Signal`，不解释含义。
把这条边界守住，加一个平台就只是加一个 provider，而不是把 if-else 抄两遍。

## 为什么不用"信号即播报"

系统的会话/电源通知有三个坑，都在这套代码里被显式处理：

1. **注册即回放**。`RegisterPowerSettingNotification` 会立刻投递当前值；
   直接把它当变化处理，程序一开机就会播报一次"电源已接入"。
   → 第一个电源样本只播种状态，不产生事件（`first_power_sample_never_broadcasts`）。
2. **同一件事有多个来源**。解锁可能同时来自 `WTS_SESSION_UNLOCK`、
   `CONSOLE_CONNECT` 和显示恢复。
   → 状态机记住"当前是否已解锁"，只有边沿才播报
   （`unlock_only_fires_on_a_real_locked_to_unlocked_edge`）。
3. **阈值附近会抖**。电量在 5% 上下跳动能连播三遍。
   → 迟滞：跌破 5% 触发，必须回到 8% 才重新武装，外加每个事件 60 秒的最小间隔
   （`battery_low_uses_hysteresis_and_fires_exactly_twice`）。

## 开机进入桌面 vs 解锁返回桌面

两个事件描述的是同一个瞬间，同时开会重复播报。判据是**服务启动到登录的间隔**：

- 服务在开机时比登录界面更早就绪，所以"输完密码进桌面"必然落在
  `boot_grace_ms`（默认 90 秒）之内 → `开机进入桌面`；
- 之后的解锁、切换回控制台 → `解锁返回桌面`。

出厂设置只开后者。想改成开机播报，把两个开关对调即可（`--enable boot_desktop
--disable unlock`）。

## 屏保恢复

Windows 没有"屏保结束"这一条通知。可用的近似是 `GUID_CONSOLE_DISPLAY_STATE`
（0 灭 / 1 亮 / 2 调光）：灭→亮且期间**没有**锁屏，就算屏保恢复；
如果期间锁了，那就是一次正常解锁，交给 `解锁返回桌面`
（`screensaver_restore_needs_display_off_then_on_while_unlocked`）。

睡眠唤醒不算屏保恢复（`resume_is_not_a_screensaver_restore`）。
这个事件默认关闭：它的判据是启发式，作者不想让一个可能误报的声音默认响。

## 低电量模式

Windows 没有稳定的 Win32 查询接口，WinRT 的
`Windows.System.Power.PowerManager.BatterySaverStatus` 在 windows-gnu 上没验证过。
当前实现是**派生值**：离线且电量 ≤ `runtime.saver_threshold_percent`（默认 20，
正好是 Windows 的默认阈值）。也就是说：系统真的进入省电模式时我们大概率同步，
但用户手动开省电模式时我们不会响。这一点在 UI 和文档里都不遮掩。

Linux 侧同理：UPower 的 `OnBattery` + 百分比派生，KDE 的
`PowerDevil` 省电模式没有统一接口。

## Linux 的总线划分

| 需要的东西 | 总线 | 谁能读 |
|---|---|---|
| login1 会话、`LockedHint`、`Class`、`PrepareForSleep` | **系统** | root / 系统级单元；普通用户可读属性 |
| UPower `OnBattery` / `Percentage` | **系统** | 任何进程 |
| GNOME / freedesktop `ScreenSaver.GetActive` | **会话** | 只有图形会话内的进程 |

所以：
- 会话级 systemd 单元（`--agent`）能拿到全部四类事件；
- 系统级守护进程拿不到屏保事件，但能拿到登录/解锁/电源 —— 这正是
  "开机进桌面"在 greeter 阶段唯一需要的部分；
- 没有会话总线时，屏保事件静默跳过，不会报错也不会误播。

轮询而不是订阅信号，是为了不引入异步运行时：400 ms 一次本地 unix socket
属性读取的开销远小于一次唤醒，而最坏延迟只有 0.4 秒。要改成订阅，只需要在
`eds-linux` 里加 match rule 并唤醒同一个 `Feed`。

## 加第九个事件

1. `eds-core::event::EventKind` 末尾加一个变体（`index()` 跟着加）。
2. `presets::EVENTS` 加一行：中文名、英文名、分组、预设文件名、出厂开关、提示语。
3. 平台 provider 里在能观测到它的地方 `emit(Signal::你的信号)`；
   `Dispatcher::apply` 里加一条边沿规则。
4. 给这条规则写一个单元测试。UI 会自动多出一行，不用改 .slint。
