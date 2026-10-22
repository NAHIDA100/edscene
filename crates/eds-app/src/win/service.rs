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
            | ServiceControlAccept::SHUTDOWN
            | ServiceControlAccept::SESSION_CHANGE
            | ServiceControlAccept::POWER_EVENT,
        exit_code: ServiceExitCode::NO_ERROR,
        checkpoint: 0,
        wait_hint: Duration::ZERO,
        process_id: None,
    }
}

fn stopped_status(failed: bool) -> ServiceStatus {
    ServiceStatus {
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: if failed { ServiceExitCode::ServiceSpecific(1) } else { ServiceExitCode::NO_ERROR },
        ..running_status()
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

/// The image to register: keep whatever the service already points at, and only
/// fall back to this executable for a fresh install.
///
/// Without this, clicking "以管理员身份开启" in a settings window that happens
/// to run from `target\release` re-points the boot-time service at a build
/// directory that can be deleted at any moment — which is also how the service
/// ends up locking its own source tree against rebuilds.
fn installed_info(mgr: &ServiceManager) -> Result<ServiceInfo, String> {
    let mut info = service_info()?;
    let mut wanted: Option<std::path::PathBuf> = None;
    if let Ok(svc) = mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_CONFIG) {
        if let Ok(cur) = svc.query_config() {
            if cur.executable_path.is_file() && !in_build_dir(&cur.executable_path) {
                wanted = Some(cur.executable_path.clone());
            }
        }
    }
    // A build-directory path (or none at all) is worse than the documented
    // install location, if something was ever installed there.
    let wanted = wanted.or_else(|| {
        let pd = std::env::var_os("ProgramFiles")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Program Files"))
            .join("edscene")
            .join("edscene.exe");
        pd.is_file().then_some(pd)
    });
    if let Some(p) = wanted {
        if p != info.executable_path {
            if in_build_dir(&info.executable_path) {
                eprintln!(
                    "提示：这次是从构建目录 {} 运行的，服务镜像改用 {p:?}，\
                     以免开机自启依赖一个随时会被删掉的目录。",
                    info.executable_path.display()
                );
            }
            info.executable_path = p;
        }
    }
    Ok(info)
}

/// What `--install-service` registers: the executable that is running this very
/// command.
///
/// Keeping the old image is wrong here — that is how a machine that once ran a
/// zip from 下载 keeps booting that zip after an install into Program Files.
/// Only a build directory is unsafe to boot from, and then the conservative
/// fallback applies.
fn install_info(mgr: &ServiceManager) -> Result<ServiceInfo, String> {
    let info = service_info()?;
    if !in_build_dir(&info.executable_path) {
        return Ok(info);
    }
    installed_info(mgr)
}

/// True for `…\target\release\edscene.exe` and friends.
fn in_build_dir(p: &Path) -> bool {
    p.ancestors()
        .any(|a| a.file_name().map(|n| n == "target").unwrap_or(false))
}

/// windows-service prints API failures as the useless "IO error in winapi
/// call"; the Win32 code behind it is the only thing worth showing.
fn werr(e: windows_service::Error) -> String {
    match e {
        windows_service::Error::Winapi(ref io) => match io.raw_os_error() {
            Some(c) => format!("{io} (os error {c})"),
            None => io.to_string(),
        },
        other => other.to_string(),
    }
}

fn state_name(s: ServiceState) -> &'static str {
    match s {
        ServiceState::Stopped => "已停止",
        ServiceState::Running => "运行中",
        ServiceState::StartPending => "正在启动",
        ServiceState::StopPending => "正在停止",
        ServiceState::Paused => "已暂停",
        _ => "过渡状态",
    }
}

/// Register (or repair) the service and start it. Needs elevation; without it
/// this re-launches itself through UAC so the user only has to click Yes.
pub fn install() -> Result<(), String> {
    let r = install_inner();
    log_attempt("--install-service", &r);
    r
}

fn install_inner() -> Result<(), String> {
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
    let info = install_info(&mgr)?;
    let svc = match mgr.create_service(&info, ServiceAccess::ALL_ACCESS) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("已存在同名服务，改为更新配置（{})", werr(e));
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
    let mut stopped = false;
    for _ in 0..40 {
        match svc.query_status() {
            Ok(s) if s.current_state == ServiceState::Stopped => {
                stopped = true;
                break;
            }
            _ => std::thread::sleep(Duration::from_millis(250)),
        }
    }
    if !stopped {
        // A service that died without reporting its state stays "RUNNING" as far
        // as the SCM is concerned, and no amount of asking will stop it.
        return Err(format!(
            "{} 的进程对停止请求毫无反应（多半是旧版假死）。\n\
             请在管理员窗口执行 taskkill /F /IM edscene.exe，然后重跑本命令。",
            SERVICE_NAME
        ));
    }
    const ALREADY_RUNNING: i32 = 1056; // ERROR_SERVICE_ALREADY_RUNNING
    if let Err(e) = svc.start::<&str>(&[]) {
        // The stop above can still be in flight, and a crash-looping service is
        // mid-restart; in both cases the SCM already considers it started.
        let already = matches!(&e, windows_service::Error::Winapi(io)
            if io.raw_os_error() == Some(ALREADY_RUNNING));
        if !already {
            return Err(format!("启动服务失败：{}", werr(e)));
        }
    }
    // StartService returns before the service gets to fail, which is exactly
    // how a broken pipe descriptor once looked like a successful install.
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(300));
        let state = svc
            .query_status()
            .map(|s| s.current_state)
            .map_err(|e| werr(e))?;
        if state != ServiceState::Running {
            let dir = machine_state_dir();
            let mut msg = format!(
                "服务启动后立刻退出了（{}/{}），原因记录在 {}，改好后重跑本命令",
                state_name(state),
                SERVICE_NAME,
                dir.display()
            );
            // The service runs in session 0 and dies without printing anywhere
            // an installer could see, but it does write its own reason. Quote it
            // here so "exit code 1" never has to be the whole story.
            if let Some(last) = last_service_error(&dir) {
                msg.push_str("\n服务自己留下的最后一条记录：");
                msg.push_str(&last);
            }
            return Err(msg);
        }
    }
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

/// Where the *service* writes, whether or not that directory existed before:
/// `machine_paths()` is only a probe, so on a fresh machine it answers "nothing
/// here" a moment before the service creates it.
fn machine_state_dir() -> std::path::PathBuf {
    paths::ensure_machine_paths()
        .map(|p| p.state_dir)
        .unwrap_or_else(|| paths::active_paths().state_dir)
}

/// The last thing the service itself managed to record, if it recorded anything.
fn last_service_error(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(log::file(dir)).ok()?;
    text.lines()
        .rev()
        .find(|l| l.contains("\"err\""))
        .map(|l| l.trim().to_string())
}

/// Every service verb also writes here, one human line per attempt.
///
/// The installer runs the child hidden and the UAC-elevated child is a GUI
/// subsystem process, so stdout has nowhere to go on both paths: without this
/// file the only thing anybody ever sees is "退出码 1". It sits beside the
/// config, not in it, so a normal user can read it but never corrupt it.
pub fn install_log_path() -> std::path::PathBuf {
    let dir = paths::ensure_machine_paths()
        .and_then(|p| p.state_dir.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| paths::user_paths().state_dir);
    let _ = std::fs::create_dir_all(&dir);
    dir.join("service-install.log")
}

fn stamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

fn log_attempt(verb: &str, r: &Result<(), String>) {
    use std::io::Write;
    let p = install_log_path();
    let new = !p.exists();
    let exe = std::env::current_exe()
        .map(|e| e.display().to_string())
        .unwrap_or_else(|_| "未知".into());
    let line = match r {
        Ok(()) => format!(
            "[{}] {verb} 返回成功（服务名 {SERVICE_NAME}，本次运行的 exe：{exe}，{}）\n",
            stamp(),
            if is_admin() {
                "本进程已提权"
            } else {
                "本进程未提权，已交给 UAC 子进程，真正的结果在下面几行"
            }
        ),
        Err(e) => format!(
            "[{}] {verb} 失败（本次运行的 exe：{exe}）：{}\n",
            stamp(),
            e.replace('\n', "\n    ")
        ),
    };
    let opened = std::fs::OpenOptions::new().create(true).append(true).open(&p);
    if let Ok(mut f) = opened {
        if new {
            // Notepad needs the BOM to show Chinese at all.
            let _ = f.write_all("\u{feff}".as_bytes());
        }
        let _ = f.write_all(line.as_bytes());
    }
}

pub fn uninstall() -> Result<(), String> {
    let r = uninstall_inner();
    log_attempt("--uninstall-service", &r);
    r
}

fn uninstall_inner() -> Result<(), String> {
    if !is_admin() {
        return Err("需要管理员权限：请以管理员身份运行 edscene --uninstall-service".into());
    }
    let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| e.to_string())?;
    let svc = mgr
        .open_service(SERVICE_NAME, ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS)
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
    let r = set_autostart_inner(on);
    log_attempt(if on { "--autostart-on" } else { "--autostart-off" }, &r);
    r
}

fn set_autostart_inner(on: bool) -> Result<(), String> {
    if !is_admin() {
        return Err("修改开机自启需要管理员权限".into());
    }
    let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| e.to_string())?;
    let svc = mgr
        .open_service(SERVICE_NAME, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .map_err(|e| e.to_string())?;
    let mut info = installed_info(&mgr)?;
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
    // LocalSystem must not fall back to *its own* profile: that is the
    // systemprofile directory, where the settings window never looks.
    let paths = paths::ensure_machine_paths().unwrap_or_else(paths::active_paths);
    let (sig_tx, sig_rx) = mpsc::channel::<Signal>();
    let tx = sig_tx.clone();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let handle = service_control_handler::register(SERVICE_NAME, move |ctrl| {
        match ctrl {
            // Carries no payload: it only means "re-read the status now".
            ServiceControl::PowerEvent(PowerEventParam::PowerStatusChange) => poll_power(&tx),
            // Stop and Shutdown reach no dispatcher: without this the loop
            // keeps running, the SCM waits in vain, and every reinstall has
            // to end with a taskkill.
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = stop_tx.send(());
            }
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

    let r = supervise(&paths, sig_rx, &sig_tx, stop_rx);
    // A deliberate stop must not be reported as a service failure.
    let _ = handle.set_service_status(stopped_status(r.is_err()));
    r
}

fn supervise(
    paths: &eds_core::Paths,
    sig_rx: mpsc::Receiver<Signal>,
    sig_tx: &mpsc::Sender<Signal>,
    stop_rx: mpsc::Receiver<()>,
) -> Result<(), String> {
    let server = Server::start()?;
    let mut cfg = load_at(paths);
    let mut disp = Dispatcher::new(cfg.dispatch_config(), log::mono_ms());
    disp.set_machine_uptime(crate::platform::uptime_ms());
    let mut seq: u64 = 0;
    let mut last_cfg = mtime(&paths.config);
    let mut last_cfg_load = Instant::now();
    let mut poll = Duration::from_millis(cfg.runtime.power_poll_ms.max(500));
    let mut queued: Vec<(u32, Msg, Instant)> = Vec::new();

    // A user may already be logged in when the service starts (install, or a
    // late start after boot), so greet the console session immediately.
    let console = console_session();
    if console != 0xFFFFFFFF {
        spawn_sidecars(&server, console, &cfg, paths, "startup");
    }
    let mut agents: Vec<u32> = Vec::new();

    loop {
        if stop_rx.try_recv().is_ok() {
            return Ok(());
        }
        let sig = match sig_rx.recv_timeout(poll) {
            Ok(s) => Some(s),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                poll_power(sig_tx);
                None
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        };
        while let Ok(more) = sig_rx.try_recv() {
            deliver(&mut disp, &server, &cfg, more, &mut seq, &mut agents, &mut queued, paths);
        }
        if let Some(s) = sig {
            deliver(&mut disp, &server, &cfg, s, &mut seq, &mut agents, &mut queued, paths);
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
                    flush(&server, session, &mut queued, paths);
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
                cfg = load_at(paths);
                // Keep what we learned about the power state; take the new rules.
                disp.reload_config(cfg.dispatch_config());
                poll = Duration::from_millis(cfg.runtime.power_poll_ms.max(500));
                log::write(&paths.state_dir, &serde_json::json!({"ev": "config-reload"}));
            }
        }
        // Nobody turned up to play it: stop holding it.
        queued.retain(|(_, _, at)| at.elapsed() < QUEUE_TTL);
    }
}

/// A broadcast whose session had no player yet. The agent needs a moment to
/// start, and the boot announcement is exactly the one that races it.
const QUEUE_TTL: Duration = Duration::from_secs(8);

fn flush(
    server: &Server,
    session: u32,
    queued: &mut Vec<(u32, Msg, Instant)>,
    paths: &eds_core::Paths,
) {
    while let Some(i) = queued.iter().position(|(s, _, _)| *s == session) {
        let (_, msg, at) = queued.remove(i);
        if at.elapsed() >= QUEUE_TTL {
            continue;
        }
        let kind = match &msg {
            Msg::Event { kind, .. } => *kind,
            _ => continue,
        };
        match server.send(Cmd::ToSession(session, msg)) {
            Ok(()) => log::write(
                &paths.state_dir,
                &serde_json::json!({"ev": "flush", "kind": kind.id(), "session": session,
                                    "late_ms": at.elapsed().as_millis()}),
            ),
            Err(e) => log::write(
                &paths.state_dir,
                &serde_json::json!({"ev": "send-fail", "kind": kind.id(), "err": e}),
            ),
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
    queued: &mut Vec<(u32, Msg, Instant)>,
    paths: &eds_core::Paths,
) {
    // A logon means there is now a token to launch the player with.
    if let Signal::Logon { session } = sig {
        if !agents.contains(&session) {
            spawn_sidecars(server, session, cfg, paths, "logon");
        } else if cfg.ui.tray {
            spawn_tray(session, paths);
        }
    }
    let now = log::mono_ms();
    for kind in disp.apply(sig, now) {
        if !cfg.is_enabled(kind) {
            log::write(
                &paths.state_dir,
                &serde_json::json!({"ev": "muted", "kind": kind.id(), "t": now}),
            );
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
        // The hub drops a ToSession silently when that session has no player,
        // so hold it instead: the boot announcement always loses that race.
        let attached = agents.contains(&session);
        let msg = Msg::Event {
            kind,
            seq: *seq,
            session,
            at_ms: now,
        };
        if !attached {
            queued.push((session, msg, Instant::now()));
            log::write(
                &paths.state_dir,
                &serde_json::json!({"ev": "queued", "kind": kind.id(), "seq": *seq,
                                    "session": session}),
            );
            continue;
        }
        match server.send(Cmd::ToSession(session, msg)) {
            Ok(()) => log::write(
                &paths.state_dir,
                &serde_json::json!({"ev": "fire", "kind": kind.id(), "seq": *seq,
                                    "session": session, "agent": attached}),
            ),
            Err(e) => log::write(
                &paths.state_dir,
                &serde_json::json!({"ev": "send-fail", "kind": kind.id(), "seq": *seq, "err": e}),
            ),
        }
    }
}

/// Mint the door key, hand it to the hub, *then* launch: the agent dials in
/// within milliseconds and must find the nonce already registered.
fn spawn_agent_for(server: &Server, session: u32) -> Result<u32, String> {
    let nonce = ipc::new_nonce();
    server.expect_nonce(session, &nonce)?;
    launch::spawn_agent_safe(session, &nonce)
}

/// Everything the service has to put into a freshly logged-on session: the
/// player, and the tray window if the user wants the GUI one click away.
/// Failures are logged, because session 0 has nowhere to print them.
fn spawn_sidecars(
    server: &Server,
    session: u32,
    cfg: &Config,
    paths: &eds_core::Paths,
    why: &str,
) {
    if let Err(e) = spawn_agent_for(server, session) {
        log::write(
            &paths.state_dir,
            &serde_json::json!({"ev": "spawn-fail", "session": session, "err": e, "why": why}),
        );
    }
    if cfg.ui.tray {
        spawn_tray(session, paths);
    }
}

fn spawn_tray(session: u32, paths: &eds_core::Paths) {
    let event = match launch::spawn_tray_safe(session) {
        Ok(pid) => serde_json::json!({"ev": "tray-spawn", "session": session, "pid": pid}),
        Err(e) => serde_json::json!({"ev": "tray-fail", "session": session, "err": e}),
    };
    log::write(&paths.state_dir, &event);
}

fn console_session() -> u32 {
    unsafe { windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId() }
}

fn load_at(paths: &eds_core::Paths) -> Config {
    let mut cfg = Config::load(&paths.config)
        .ok()
        .flatten()
        .unwrap_or_else(Config::factory);
    // 随包的 sounds\ 就在 exe 旁边。配置里那个目录不存在（换机器、重装、盘符变了）
    // 时读时用这一份 —— 只改内存里的值，不回写配置：一个暂时离线的网络盘不该被
    // 服务从 session 0 里覆盖掉。
    if cfg.sounds.dir.is_empty() || !std::path::Path::new(&cfg.sounds.dir).is_dir() {
        if let Some(dir) = crate::paths::shipped_sounds() {
            cfg.sounds.dir = dir.display().to_string();
        }
    }
    cfg
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
