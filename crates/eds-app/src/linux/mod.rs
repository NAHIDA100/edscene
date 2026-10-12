//! Linux half of the binary: the D-Bus provider loop, the autostart toggle and
//! the backdrop. The rules all live in `eds_core`; this file only moves bytes
//! between the OS and them.

use crate::log;
use crate::pipeline::{Broadcast, LocalBroadcast, NullBroadcast, Pipeline};
use crate::{load_config, paths, Invocation};
use eds_core::config::Config;
use eds_core::presets::def;
use eds_core::Paths;
use eds_linux::{desktop, Buses, Feed};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub fn watch(_inv: &Invocation) -> Result<(), String> {
    let p = paths::active_paths();
    let (cfg, _) = load_config(&p)?;
    println!(
        "监听中（不出声），配置 {}\nCtrl+C 退出。",
        p.config.display()
    );
    run_provider(&cfg, Arc::new(NullBroadcast), true)
}

pub fn agent(_inv: &Invocation) -> Result<(), String> {
    let p = paths::active_paths();
    let (cfg, _) = load_config(&p)?;
    let out = Arc::new(LocalBroadcast::new(cfg.clone(), p.clone()));
    spawn_config_watch(p.clone(), out.clone());
    run_provider(&cfg, out, false)
}

/// Poll the buses on one thread and feed the shared dispatcher. A 400 ms tick
/// keeps the greeting inside one heartbeat without needing an async runtime.
fn run_provider(cfg: &Config, out: Arc<dyn Broadcast>, verbose: bool) -> Result<(), String> {
    let buses = Buses::open()?;
    let pipe = Arc::new(Pipeline::new(cfg, out));
    // Both entry points run inside an already-unlocked desktop session.
    pipe.assume_desktop();
    if verbose {
        println!(
            "系统总线已连接；会话总线{}",
            if buses.user.is_some() { "已连接" } else { "不可用（屏保事件将静默）" }
        );
    }
    let (tx, rx) = std::sync::mpsc::channel::<eds_core::Signal>();
    std::thread::Builder::new()
        .name("edscene-dbus".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let mut feed = Feed::default();
            while !STOPPED.load(Ordering::Relaxed) {
                feed.push(buses.poll(), &tx);
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        })
        .map_err(|e| e.to_string())?;
    while let Ok(sig) = rx.recv() {
        let at = log::mono_ms();
        if verbose {
            println!("[{at:>9} ms] {sig:?}");
        }
        for k in pipe.feed(sig, at) {
            if verbose {
                println!("            ↳ 播报 {}", def(k).title_zh);
            }
        }
    }
    Ok(())
}

static STOPPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn request_stop() {
    STOPPED.store(true, Ordering::Relaxed);
}

fn spawn_config_watch(p: Paths, out: Arc<LocalBroadcast>) {
    std::thread::Builder::new()
        .name("edscene-cfg".into())
        .stack_size(96 * 1024)
        .spawn(move || {
            let mut last: Option<(std::time::SystemTime, u64)> = None;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(2_000));
                let Ok(md) = std::fs::metadata(&p.config) else { continue };
                let stamp = (
                    md.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                    md.len(),
                );
                // Any change (including the first sample) reloads, so an agent
                // started before the UI finished its first save still catches up.
                if last.replace(stamp).is_some() {
                    if let Ok((cfg, _)) = load_config(&p) {
                        out.set_config(cfg);
                    }
                }
            }
        })
        .ok();
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let touched = desktop::write_launch_files(&exe, on)?;
    if touched.is_empty() {
        return Err("没有可写的自启文件（检查 $XDG_CONFIG_HOME）".into());
    }
    // Best effort: a session with no user bus still gets the XDG entry, and
    // systemd-based desktops get the unit enabled here.
    let _ = Command::new("systemctl")
        .args(desktop::systemctl_args(on))
        .status();
    if on && !whoami().is_empty() {
        let _ = Command::new("loginctl")
            .arg("enable-linger")
            .arg(whoami())
            .status();
    }
    Ok(())
}

fn whoami() -> String {
    std::env::var("USER").unwrap_or_default()
}

pub fn unit_installed() -> bool {
    desktop::unit_installed()
}

/// 壁纸副本 / 实时模糊 on Linux: ask the desktop where its wallpaper is, and if
/// that fails try a screenshot tool. Both end up through the same blur as
/// Windows, so the backdrop looks the same on either platform.
pub fn blurred_wallpaper() -> Option<slint::Image> {
    for path in desktop::wallpaper_candidates() {
        if let Some(img) = crate::backdrop::soften_path(&path) {
            return Some(img);
        }
    }
    for argv in desktop::screenshot_argv(eds_linux::is_wayland()) {
        if let Some(img) = grab(argv) {
            return Some(img);
        }
    }
    None
}

fn grab(argv: &[&'static str]) -> Option<slint::Image> {
    let (head, rest) = argv.split_first()?;
    let out = Command::new(head).args(rest).output().ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    crate::backdrop::soften_bytes(&out.stdout)
}

/// Native pickers: the desktop's own tool, whichever of the two it ships.
pub fn pick_file() -> Option<PathBuf> {
    pick(&[
        &["zenity", "--file-selection"][..],
        &["kdialog", "--getopenfilename"][..],
    ])
}

pub fn pick_folder() -> Option<PathBuf> {
    pick(&[
        &["zenity", "--file-selection", "--directory"][..],
        &["kdialog", "--getexistingdirectory"][..],
    ])
}

fn pick(argvs: &[&[&str]]) -> Option<PathBuf> {
    for argv in argvs {
        let Ok(out) = Command::new(argv[0]).args(&argv[1..]).output() else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(PathBuf::from(s));
        }
    }
    None
}

pub fn open_dir(p: &Path) {
    let _ = Command::new("xdg-open").arg(p).spawn();
}
