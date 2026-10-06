//! The Windows service: the process that is already loaded while the user is
//! still typing their password.
//!
//! It owns nothing it can use twice — no audio device, no window — because it
//! runs in session 0. What it owns is the *decision*: it watches sessions and
//! power, runs the dispatcher, and tells the agent living in the interactive
//! session what to play.

use super::ipc::{self, Cmd, Server, ServiceEvent};
use super::launch;
use crate::log;
use crate::paths;
use eds_core::config::Config;
use eds_core::dispatcher::Dispatcher;
use eds_core::ipc::Msg;
use eds_core::Signal;
use std::ffi::OsString;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows_service::service::{
    PowerEventParam, ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl,
    ServiceExitCode, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    SessionChangeParam, SessionChangeReason,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::service_dispatcher;

pub const SERVICE_NAME: &str = "edscene-broadcast";
pub const DISPLAY_NAME: &str = "集成工业播报终端";
pub const DESCRIPTION: &str =
    "在登录、解锁与电源变化时播报集成工业语音。服务本身不出声，出声的是各会话里的代理进程。";

fn running_status() -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP
            | ServiceControlAccept::SESSION_CHANGE
            | ServiceControlAccept::POWER_EVENT,
        exit_code: ServiceExitCode::NO_ERROR,
        checkpoint: 0,
        wait_hint: Duration::ZERO,
        process_id: None,
    }
}

fn service_info() -> Result<ServiceInfo, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![OsString::from("--service")],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    })
}

/// Register (or repair) the service and start it. Needs elevation; without it
/// this re-launches itself through UAC so the user only has to click Yes.
pub fn install() -> Result<(), String> {
    if !is_admin() {
        return match elevate("--install-service") {
            Ok(()) => {
                println!("已在管理员窗口里继续安装，请看 UAC 弹窗。");
                Ok(())
            }
            Err(e) => Err(format!("{e}\n也可以手动以管理员运行：edscene --install-service")),
        };
    }
    let mgr = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(|e| e.to_string())?;
    let info = service_info()?;
    let svc = match mgr.create_service(&info, ServiceAccess::ALL_ACCESS) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("已存在同名服务，改为更新配置（{e}）");
            mgr.open_service(SERVICE_NAME, ServiceAccess::ALL_ACCESS)
                .map_err(|e| e.to_string())?
                .change_config(&info)
                .map_err(|e| e.to_string())?;
            mgr.open_service(SERVICE_NAME, ServiceAccess::ALL_ACCESS | ServiceAccess::START)
                .map_err(|e| e.to_string())?
        }
    };
    if let Err(e) = describe_and_harden(&svc) {
        eprintln!("（服务描述/失败重启策略未写入：{e}）");
    }
    // Restart rather than merely start: after an upgrade the old image is
    // still resident, and "start" on a running service is a silent no-op.
    let _ = svc.stop();
    for _ in 0..40 {
        match svc.query_status() {
            Ok(s) if s.current_state == ServiceState::Stopped => break,
            _ => std::thread::sleep(Duration::from_millis(250)),
        }
    }
    svc.start::<&str>(&[]).map_err(|e| format!("启动服务失败：{e}"))?;
    println!(
        "服务已安装并启动：{SERVICE_NAME}\n开机自启已打开；卸载用 edscene --uninstall-service"
    );
    Ok(())
}

/// Set the human-readable description and make the service restart itself,
/// because a panic in session 0 has nowhere to print.
fn describe_and_harden(svc: &windows_service::service::Service) -> Result<(), String> {
    use windows::core::PWSTR;
    use windows::Win32::System::Services::{
        ChangeServiceConfig2W, SC_ACTION, SERVICE_FAILURE_ACTIONSW,
    };
    let desc = wide(DESCRIPTION);
    let handle = windows::Win32::System::Services::SC_HANDLE(svc.raw_handle() as *mut _);
    unsafe {
        let d = windows::Win32::System::Services::SERVICE_DESCRIPTIONW {
            lpDescription: PWSTR(desc.as_ptr() as *mut u16),
        };
        ChangeServiceConfig2W(
            handle,
            windows::Win32::System::Services::SERVICE_CONFIG_DESCRIPTION,
            Some(&d as *const _ as *const core::ffi::c_void),
        )
        .map_err(|e| e.to_string())?;
        // Restart quickly at first, then back off; give up after that.
        let acts = [
            SC_ACTION {
                Type: windows::Win32::System::Services::SC_ACTION_RESTART,
                Delay: 5_000,
            },
            SC_ACTION {
                Type: windows::Win32::System::Services::SC_ACTION_RESTART,
                Delay: 30_000,
            },
            SC_ACTION {
                Type: windows::Win32::System::Services::SC_ACTION_RESTART,
                Delay: 60_000,
            },
        ];
        let fa = SERVICE_FAILURE_ACTIONSW {
            dwResetPeriod: 86_400,
            lpRebootMsg: PWSTR::null(),
            lpCommand: PWSTR::null(),
            cActions: acts.len() as u32,
            lpsaActions: acts.as_ptr() as *mut _,
        };
        ChangeServiceConfig2W(
            handle,
            windows::Win32::System::Services::SERVICE_CONFIG_FAILURE_ACTIONS,
            Some(&fa as *const _ as *const core::ffi::c_void),
        )
        .map_err(|e| e.to_string())
    }
}

pub fn uninstall() -> Result<(), String> {
    if !is_admin() {
        return Err("需要管理员权限：请以管理员身份运行 edscene --uninstall-service".into());
    }
    let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| e.to_string())?;
    let svc = mgr
        .open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::DELETE)
        .map_err(|e| format!("没有安装服务：{e}"))?;
    let _ = svc.stop();
    for _ in 0..40 {
        match svc.query_status() {
            Ok(s) if s.current_state == ServiceState::Stopped => break,
            _ => std::thread::sleep(Duration::from_millis(250)),
        }
    }
    svc.delete().map_err(|e| e.to_string())?;
    println!("服务已卸载。用户目录里的配置与替换素材保留。");
    Ok(())
}

/// Flip the service between automatic and manual start; this is what the UI's
/// 开机自启 switch does, so there is exactly one source of truth.
pub fn set_autostart(on: bool) -> Result<(), String> {
    if !is_admin() {
        return Err("修改开机自启需要管理员权限".into());
    }
    let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| e.to_string())?;
    let svc = mgr
        .open_service(SERVICE_NAME, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .map_err(|e| e.to_string())?;
    let mut info = service_info()?;
    info.start_type = if on {
        ServiceStartType::AutoStart
    } else {
        ServiceStartType::OnDemand
    };
    svc.change_config(&info).map_err(|e| e.to_string())?;
    if on {
        let _ = svc.start::<&str>(&[]);
    }
    Ok(())
}

/// Does the service exist? The UI's 开机自启 row shows this without needing
/// elevation, unlike writing to it.
pub fn installed() -> bool {
    ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .and_then(|mgr| {
            mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
                .map_err(|e| windows_service::Error::Winapi(std::io::Error::new(
                    std::io::ErrorKind::Other, e.to_string(),
                )))
        })
        .is_ok()
}

pub fn status() -> Result<(), String> {
    let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| e.to_string())?;
    match mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
        Ok(s) => {
            let st = s.query_status().map_err(|e| e.to_string())?;
            println!("服务 {SERVICE_NAME}: {:?}", st.current_state);
            Ok(())
        }
        Err(e) => {
            println!("服务未安装（{e}）");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------- service body

pub fn run() -> Result<(), String> {
    service_dispatcher::start(OsString::from(SERVICE_NAME), ffi_main).map_err(|e| e.to_string())
}

extern "system" fn ffi_main(_argc: u32, _argv: *mut *mut u16) {
    if let Err(e) = body() {
        // Nothing can show a dialog in session 0; the log is the only channel.
        let p = paths::ensure_machine_paths().unwrap_or_else(paths::active_paths);
        log::write(
            &p.state_dir,
            &serde_json::json!({"ev": "service-exit", "err": e.to_string()}),
        );
    }
}

fn body() -> Result<(), String> {
    let paths = paths::machine_paths().unwrap_or_else(paths::active_paths);
    let cfg = load_at(&paths);
    let (sig_tx, sig_rx) = mpsc::channel::<Signal>();
    let tx = sig_tx.clone();
    let handle = service_control_handler::register(SERVICE_NAME, move |ctrl| {
        match ctrl {
            // Carries no payload: it only means "re-read the status now".
            ServiceControl::PowerEvent(PowerEventParam::PowerStatusChange) => poll_power(&tx),
            other => {
                if let Some(s) = translate(other) {
                    let _ = tx.send(s);
                }
            }
        }
        ServiceControlHandlerResult::NoError
    })
    .map_err(|e| e.to_string())?;
    handle
        .set_service_status(running_status())
        .map_err(|e| e.to_string())?;

    let server = Server::start()?;
    let mut disp = Dispatcher::new(cfg.dispatch_config(), log::mono_ms());
    let mut seq: u64 = 0;
    let mut last_cfg = mtime(&paths.config);
    let mut last_cfg_load = Instant::now();
    let mut poll = Duration::from_millis(cfg.runtime.power_poll_ms.max(500));

    // A user may already be logged in when the service starts (install, or a
    // late start after boot), so greet the console session immediately.
    let console = console_session();
    if console != 0xFFFFFFFF {
        let _ = spawn_agent_for(&server, console);
    }
    let mut agents: Vec<u32> = Vec::new();

    loop {
        let sig = match sig_rx.recv_timeout(poll) {
            Ok(s) => Some(s),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                poll_power(&sig_tx);
                None
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        };
        while let Ok(more) = sig_rx.try_recv() {
            deliver(&mut disp, &server, &cfg, more, &mut seq, &mut agents, &paths);
        }
        if let Some(s) = sig {
            deliver(&mut disp, &server, &cfg, s, &mut seq, &mut agents, &paths);
        }
        while let Ok(ev) = server.events.try_recv() {
            match ev {
                ServiceEvent::AgentConnected { session, pid } => {
                    if !agents.contains(&session) {
                        agents.push(session);
                    }
                    log::write(
                        &paths.state_dir,
                        &serde_json::json!({"ev": "agent", "session": session, "pid": pid}),
                    );
                }
                ServiceEvent::AgentGone { session } => {
                    agents.retain(|s| *s != session);
                    // A dead agent's key must not outlive it.
                    server.forget_session(session);
                }
            }
        }
        // The UI writes the file; reload by timestamp, no watcher needed.
        if last_cfg_load.elapsed() > Duration::from_secs(2) {
            last_cfg_load = Instant::now();
            let m = mtime(&paths.config);
            if m != last_cfg {
                last_cfg = m;
                poll = Duration::from_millis(cfg.runtime.power_poll_ms.max(500));
            }
        }
    }
}

fn translate(ctrl: ServiceControl) -> Option<Signal> {
        match ctrl {
        ServiceControl::SessionChange(p) => {
            let SessionChangeParam {
                reason,
                notification,
            } = p;
            let session = notification.session_id;
            Some(match reason {
                SessionChangeReason::SessionLogon => Signal::Logon { session },
                SessionChangeReason::SessionUnlock => Signal::Unlock { session },
                SessionChangeReason::SessionLock => Signal::Lock { session },
                SessionChangeReason::SessionLogoff => Signal::Logoff { session },
                SessionChangeReason::ConsoleConnect | SessionChangeReason::RemoteConnect => {
                    Signal::ConsoleConnect { session }
                }
                SessionChangeReason::ConsoleDisconnect => Signal::ConsoleDisconnect,
                _ => return None,
            })
        }
        ServiceControl::PowerEvent(PowerEventParam::Suspend) => Some(Signal::Suspending),
        ServiceControl::PowerEvent(PowerEventParam::ResumeAutomatic)
        | ServiceControl::PowerEvent(PowerEventParam::ResumeSuspend)
        | ServiceControl::PowerEvent(PowerEventParam::ResumeCritical) => Some(Signal::Resuming),
        _ => None,
    }
}

/// `PowerStatusChange` carries no payload, so it is only a wake-up: read the
/// authoritative status instead of trusting a placeholder.
fn poll_power(tx: &mpsc::Sender<Signal>) {
    let mut st = windows::Win32::System::Power::SYSTEM_POWER_STATUS::default();
    if unsafe { windows::Win32::System::Power::GetSystemPowerStatus(&mut st) }.is_err() {
        return;
    }
    let percent = if st.BatteryLifePercent == 255 {
        100
    } else {
        st.BatteryLifePercent
    };
    let _ = tx.send(Signal::Power {
        on_ac: st.ACLineStatus != 0,
        percent,
        saver: None,
    });
}

fn deliver(
    disp: &mut Dispatcher,
    server: &Server,
    cfg: &Config,
    sig: Signal,
    seq: &mut u64,
    agents: &mut Vec<u32>,
    paths: &eds_core::Paths,
) {
    // A logon means there is now a token to launch the player with.
    if let Signal::Logon { session } = sig {
        if !agents.contains(&session) {
            if let Err(e) = spawn_agent_for(server, session) {
                log::write(
                    &paths.state_dir,
                    &serde_json::json!({"ev": "spawn-fail", "session": session, "err": e}),
                );
            }
        }
    }
    let now = log::mono_ms();
    for kind in disp.apply(sig, now) {
        if !cfg.is_enabled(kind) {
            continue;
        }
        *seq += 1;
        let session = match sig {
            Signal::Logon { session }
            | Signal::Unlock { session }
            | Signal::Lock { session }
            | Signal::ConsoleConnect { session } => session,
            _ => console_session(),
        };
        let _ = server.send(Cmd::ToSession(
            session,
            Msg::Event {
                kind,
                seq: *seq,
                session,
                at_ms: now,
            },
        ));
    }
}

/// Mint the door key, hand it to the hub, *then* launch: the agent dials in
/// within milliseconds and must find the nonce already registered.
fn spawn_agent_for(server: &Server, session: u32) -> Result<u32, String> {
    let nonce = ipc::new_nonce();
    server.expect_nonce(session, &nonce)?;
    launch::spawn_agent_safe(session, &nonce)
}

fn console_session() -> u32 {
    unsafe { windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId() }
}

fn load_at(paths: &eds_core::Paths) -> Config {
    Config::load(&paths.config)
        .ok()
        .flatten()
        .unwrap_or_else(Config::factory)
}

fn mtime(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).ok().and_then(|m| m.modified().ok())
}

// ------------------------------------------------------------------ elevation

/// True when this process has an elevated token.
pub fn is_admin() -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elev as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(token);
        ok && elev.TokenIsElevated != 0
    }
}

/// Re-run this same command through the UAC prompt.
pub fn elevate(args: &str) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_w = wide(&exe.to_string_lossy());
    let verb = wide("runas");
    let params = wide(args);
    let r = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(exe_w.as_ptr()),
            PCWSTR(params.as_ptr()),
            None,
            SW_SHOW,
        )
    };
    if r.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("UAC 提权未成功（代码 {:?}）", r.0))
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
