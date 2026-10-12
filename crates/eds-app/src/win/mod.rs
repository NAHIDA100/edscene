//! Windows event sources.
//!
//! Two cooperating halves:
//! * a message-only window that receives `WM_WTSSESSION_CHANGE` and
//!   `WM_POWERSETTINGCHANGE`, and
//! * a slow poll of `GetSystemPowerStatus`, which is the authoritative source
//!   for AC state and charge level (the notifications are only wake-ups).
//!
//! Both feed raw [`Signal`]s into the shared dispatcher, so nothing in here
//! interprets: adding a platform means adding a provider, not new rules.

pub mod dialogs;
pub mod ipc;
pub mod launch;
pub mod service;

use crate::log;
use crate::pipeline::{Broadcast, LocalBroadcast, NullBroadcast, Pipeline};
use eds_core::ipc::{Msg, SeqGuard};
use crate::{load_config, paths, Invocation};
use eds_core::config::Config;
use eds_core::presets::def;
use eds_core::Signal;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Power::{
    GetSystemPowerStatus, RegisterPowerSettingNotification, UnregisterPowerSettingNotification,
    HPOWERNOTIFY, POWERBROADCAST_SETTING, SYSTEM_POWER_STATUS,
};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows::Win32::System::SystemServices::{
    GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING, GUID_CONSOLE_DISPLAY_STATE,
    GUID_SESSION_DISPLAY_STATUS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    PostQuitMessage, RegisterClassExW, TranslateMessage,
    DEVICE_NOTIFY_WINDOW_HANDLE, HWND_MESSAGE, MSG, WM_DESTROY, WM_WTSSESSION_CHANGE,
    WNDCLASSEXW, WTS_CONSOLE_CONNECT, WTS_REMOTE_CONNECT, WTS_SESSION_LOCK, WTS_SESSION_LOGOFF,
    WTS_SESSION_LOGON, WTS_SESSION_UNLOCK,
};

const WM_POWERSETTINGCHANGE: u32 = 0x0218;
const NOTIFY_CURRENT_SESSION: u32 = 1;

/// Set by the console control handler so `--watch` stops on Ctrl+C.
static QUIT: AtomicBool = AtomicBool::new(false);
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

struct Ctx {
    pipe: Arc<Pipeline>,
    verbose: bool,
}

static CTX: OnceLock<Ctx> = OnceLock::new();

fn emit(sig: Signal) {
    let now = log::mono_ms();
    let Some(ctx) = CTX.get() else { return };
    if ctx.verbose {
        println!("[{:>9} ms] {sig:?}", now);
    }
    for k in ctx.pipe.feed(sig, now) {
        if ctx.verbose {
            println!("            ↳ 播报 {}", def(k).title_zh);
        }
    }
}

/// Read the power status and turn it into a signal. Cheap enough to run on a
/// timer, and it is the only source that cannot miss a transition.
fn poll_power_once() {
    let mut st = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut st) }.is_err() {
        return;
    }
    let on_ac = st.ACLineStatus != 0;
    // 255 means "unknown"; 100 is the harmless value for a desktop with no
    // battery, which keeps the low-charge rules from ever arming there.
    let percent = if st.BatteryLifePercent == 255 {
        100
    } else {
        st.BatteryLifePercent
    };
    emit(Signal::Power {
        on_ac,
        percent,
        saver: None,
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_WTSSESSION_CHANGE => {
            let session = lp.0 as u32;
            let sig = match wp.0 as u32 {
                WTS_SESSION_LOGON => Some(Signal::Logon { session }),
                WTS_SESSION_UNLOCK => Some(Signal::Unlock { session }),
                WTS_SESSION_LOCK => Some(Signal::Lock { session }),
                WTS_SESSION_LOGOFF => Some(Signal::Logoff { session }),
                WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => Some(Signal::ConsoleConnect { session }),
                _ => None,
            };
            if let Some(s) = sig {
                emit(s);
            }
            LRESULT(0)
        }
        WM_POWERSETTINGCHANGE => {
            let p = lp.0 as *const POWERBROADCAST_SETTING;
            if p.is_null() {
                return LRESULT(0);
            }
            let guid = (*p).PowerSetting;
            // POWERBROADCAST_SETTING is GUID(16) + DataLength(4) + payload.
            let value = (lp.0 as *const u8).add(20) as *const u32;
            let value = value.read_unaligned();
            if guid == GUID_CONSOLE_DISPLAY_STATE || guid == GUID_SESSION_DISPLAY_STATUS {
                match value {
                    0 => emit(Signal::DisplayOff),
                    1 => emit(Signal::DisplayOn),
                    _ => {}
                }
            }
            if guid == GUID_ACDC_POWER_SOURCE
                || guid == GUID_BATTERY_PERCENTAGE_REMAINING
                || guid == GUID_CONSOLE_DISPLAY_STATE
            {
                poll_power_once();
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Run the provider until the console control handler asks it to stop.
pub fn run_provider(cfg: &Config, out: Arc<dyn Broadcast>, verbose: bool) -> Result<(), String> {
    let pipe = Arc::new(Pipeline::new(cfg, out));
    pipe.assume_desktop();
    CTX.set(Ctx {
        pipe,
        verbose,
    })
    .map_err(|_| "事件源已经在运行".to_string())?;

    let poll_ms = cfg.runtime.power_poll_ms.max(500);
    std::thread::Builder::new()
        .name("edscene-power".into())
        .stack_size(128 * 1024)
        .spawn(move || {
            while !QUIT.load(Ordering::Relaxed) {
                poll_power_once();
                std::thread::sleep(std::time::Duration::from_millis(poll_ms));
            }
        })
        .map_err(|e| e.to_string())?;

    unsafe { pump_messages() }
}

unsafe fn pump_messages() -> Result<(), String> {
    let hinstance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
    let class = w!("edscene.message.window");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinstance.into(),
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    if RegisterClassExW(&wc) == 0 {
        return Err("无法注册消息窗口类".into());
    }
    let hwnd = CreateWindowExW(
        Default::default(),
        PCWSTR(class.as_ptr()),
        w!("edscene"),
        Default::default(),
        0,
        0,
        0,
        0,
        Some(HWND_MESSAGE),
        None,
        Some(hinstance.into()),
        None,
    )
    .map_err(|e| format!("无法创建消息窗口: {e}"))?;
    MAIN_HWND.store(hwnd.0 as isize, Ordering::Relaxed);

    WTSRegisterSessionNotification(hwnd, NOTIFY_CURRENT_SESSION)
        .map_err(|e| format!("无法订阅会话变化: {e}"))?;
    let mut notes: Vec<HPOWERNOTIFY> = Vec::new();
    for guid in [
        GUID_CONSOLE_DISPLAY_STATE,
        GUID_SESSION_DISPLAY_STATUS,
        GUID_ACDC_POWER_SOURCE,
        GUID_BATTERY_PERCENTAGE_REMAINING,
    ] {
        if let Ok(n) = RegisterPowerSettingNotification(
            windows::Win32::Foundation::HANDLE(hwnd.0),
            &guid,
            DEVICE_NOTIFY_WINDOW_HANDLE,
        ) {
            notes.push(n);
        }
    }
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        if QUIT.load(Ordering::Relaxed) {
            break;
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    for n in notes {
        let _ = UnregisterPowerSettingNotification(n);
    }
    WTSUnRegisterSessionNotification(hwnd).ok();
    DestroyWindow(hwnd).ok();
    Ok(())
}

pub fn watch(_inv: &Invocation) -> Result<(), String> {
    let p = paths::active_paths();
    let (cfg, _) = load_config(&p)?;
    println!(
        "监听中（不出声），配置 {}\n可用 --quiet 之外的字段见 --help。Ctrl+C 退出。",
        p.config.display()
    );
    run_provider(&cfg, Arc::new(NullBroadcast), true)
}

/// Started by the service with `--session <n>`: plays what the service decides,
/// reloads the config by timestamp, and owns no OS watchers of its own.
pub fn agent_piped(session: u32, nonce: &str) -> Result<(), String> {
    let p = paths::active_paths();
    let (cfg, _) = load_config(&p)?;
    let out = std::sync::Arc::new(LocalBroadcast::new(cfg.clone(), p.clone()));
    spawn_config_watch(p.clone(), out.clone());
    let mut guard = SeqGuard::new();
    let backoff = std::time::Duration::from_secs(2);
    loop {
        match ipc::Client::connect(session, std::process::id(), nonce) {
            Ok(mut client) => {
                eprintln!("edscene agent: 已连接服务（会话 {session}）");
                loop {
                    match client.next() {
                        Some(Msg::Event {
                            kind,
                            seq,
                            at_ms,
                            ..
                        }) => {
                            if guard.accept(seq) {
                                out.fire(kind, seq, at_ms);
                            }
                        }
                        Some(Msg::Shutdown { reason }) => {
                            eprintln!("edscene agent: 服务要求退出（{reason}）");
                            return Ok(());
                        }
                        Some(Msg::ConfigChanged { .. }) => out.reload_config(),
                        Some(_) => {}
                        None => break,
                    }
                }
                eprintln!("edscene agent: 与服务断开，稍后重连");
            }
            Err(e) => {
                eprintln!("edscene agent: {e}");
                return Err("无法连接服务；请改用 --agent 的独立模式".into());
            }
        }
        std::thread::sleep(backoff);
    }
}

/// Cheap mtime poll: the UI owns the file, the agent only follows it.
fn spawn_config_watch(p: eds_core::Paths, out: std::sync::Arc<LocalBroadcast>) {
    std::thread::Builder::new()
        .name("edscene-cfg".into())
        .stack_size(96 * 1024)
        .spawn(move || {
            let mut last: Option<(std::time::SystemTime, u64)> = None;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(2_000));
                let Ok(md) = std::fs::metadata(&p.config) else { continue };
                let stamp = (md.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH), md.len());
                if last.replace(stamp) != Some(stamp) {
                    if let Ok((cfg, _)) = crate::load_config(&p) {
                        out.set_config(cfg);
                    }
                }
            }
        })
        .ok();
}

pub fn agent(inv: &Invocation) -> Result<(), String> {
    if let Some(session) = inv.session {
        return agent_piped(session, inv.nonce.as_deref().unwrap_or("")); 
    }
    let p = paths::active_paths();
    let (cfg, _) = load_config(&p)?;
    let out = Arc::new(LocalBroadcast::new(cfg.clone(), p.clone()));
    eprintln!(
        "edscene agent: {} 个播报已启用，素材 {}",
        cfg.enabled_events().len(),
        if cfg.sounds.dir.is_empty() {
            "（未设置）".to_string()
        } else {
            cfg.sounds.dir.clone()
        }
    );
    run_provider(&cfg, out, false)
}

pub fn service(inv: &Invocation) -> Result<(), String> {
    match inv.cmd.as_str() {
        "service" => service::run(),
        "install-service" => service::install(),
        "uninstall-service" => service::uninstall(),
        other => Err(format!("未知服务命令 {other}")),
    }
}
