mod backdrop;
mod log;
mod paths;
mod pipeline;
mod player;
mod platform;
mod ui;

use eds_core::config::Config;
use eds_core::event::EventKind;
use eds_core::presets::{def, EVENTS};
use eds_core::resolve::{self, SoundStatus};
use eds_core::{Dispatcher, Signal};
use pipeline::{Broadcast, LocalBroadcast, NullBroadcast, Pipeline};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const TARGET: &str = env!("EDSCENE_TARGET");

const HELP: &str = r#"集成工业播报终端 edscene

用法: edscene [命令] [选项]

命令
  (无)                 打开设置窗口
  --selftest [事件]     检查 8 个预设的素材是否就绪；带事件名时顺便试听
  --simulate 事件[,…]   不等系统事件，直接按顺序播报（走完整链路）
  --signals <文件>      把一份信号脚本喂进状态机，验证判定逻辑
  --watch               打印系统原始信号与播报决定，不出声
  --agent               以后台代理身份运行（由服务在登录时拉起）
  --service             以 Windows 服务身份运行（由服务控制管理器启动）
  --install-service     注册并启动开机自启服务（需要管理员）
  --uninstall-service   停止并移除服务
  --autostart-on / --autostart-off
                        开 / 关开机自启（Windows 改服务启动类型，Linux 写用户单元）
  --factory-reset       所有设置回到出厂状态，替换过的素材退回 .disabled
  --config-path         打印配置与素材目录
  --service-status      Windows：查看开机自启服务的状态
  --tray                打开设置窗口，但关到托盘时不退出（常驻托盘用）
  --version | --help

选项
  --file <路径>         --selftest 试听时改用指定音频文件
  --sounds <路径>       指定素材目录（会写回配置）
  --interval <毫秒>     --simulate 两条播报之间的间隔，默认 2500
  --json                机器可读输出
  --quiet               只输出结论行，不出声
"#;

pub struct Invocation {
    pub cmd: String,
    pub event: Option<String>,
    pub file: Option<PathBuf>,
    pub sounds: Option<String>,
    pub json: bool,
    pub quiet: bool,
    pub interval_ms: u64,
    pub script: Option<PathBuf>,
    pub session: Option<u32>,
    pub nonce: Option<String>,
    pub on: bool,
    pub hidden: bool,
}

impl Invocation {
    fn new() -> Invocation {
        Invocation {
            cmd: String::new(),
            event: None,
            file: None,
            sounds: None,
            json: false,
            quiet: false,
            interval_ms: 2_500,
            script: None,
            session: None,
            nonce: None,
            on: true,
            hidden: false,
        }
    }
}

fn parse_args() -> Result<Invocation, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut inv = Invocation::new();
    let mut i = 0;
    // Commands first, options anywhere.
    while i < argv.len() {
        let a = argv[i].clone();
        let mut next = |name: &str| -> Result<String, String> {
            let v = argv
                .get(i + 1)
                .cloned()
                .ok_or_else(|| format!("{name} 需要一个值"))?;
            i += 1;
            Ok(v)
        };
        const COMMANDS: &[(&str, &str)] = &[
            ("--help", "help"),
            ("-h", "help"),
            ("--version", "version"),
            ("-V", "version"),
            ("--selftest", "selftest"),
            ("--watch", "watch"),
            ("--agent", "agent"),
            ("--service", "service"),
            ("--install-service", "install-service"),
            ("--uninstall-service", "uninstall-service"),
            ("--factory-reset", "factory-reset"),
            ("--config-path", "config-path"),
            ("--service-status", "service-status"),
            ("--autostart-on", "autostart-on"),
            ("--autostart-off", "autostart-off"),
        ];
        if let Some((_, who)) = COMMANDS.iter().find(|(flag, _)| *flag == a.as_str()) {
            if inv.cmd.is_empty() {
                inv.cmd = (*who).to_string();
            }
        } else {
            match a.as_str() {
                "--simulate" => {
                    inv.cmd = "simulate".into();
                    inv.event = Some(next("--simulate <事件>")?);
                }
                "--signals" => {
                    inv.cmd = "signals".into();
                    inv.script = Some(PathBuf::from(next("--signals <文件>")?));
                }
                "--file" => inv.file = Some(PathBuf::from(next("--file")?)),
                "--sounds" => inv.sounds = Some(next("--sounds")?),
                "--interval" => {
                    let v = next("--interval")?;
                    inv.interval_ms = v
                        .parse::<u64>()
                        .map_err(|_| "--interval 要是整数毫秒".to_string())?;
                }
                "--session" => {
                    let v = next("--session")?;
                    inv.session = Some(v.parse().map_err(|_| "--session 要是整数".to_string())?);
                }
                "--agent-nonce" => inv.nonce = Some(next("--agent-nonce")?),
                "--enable" | "--disable" => {
                    inv.cmd = "toggle".into();
                    inv.on = a == "--enable";
                    inv.event = Some(next("--enable/--disable <事件>")?);
                }
                "--json" => inv.json = true,
                "--quiet" => inv.quiet = true,
                "--hidden" | "--tray" => inv.hidden = true,
                other => {
                    if other.starts_with('-') {
                        return Err(format!("未知参数 {other}"));
                    }
                    if inv.cmd.is_empty() {
                        inv.cmd = "selftest".into();
                        inv.event = Some(other.to_string());
                    } else if inv.event.is_none() {
                        inv.event = Some(other.to_string());
                    } else {
                        return Err(format!("多余参数 {other}"));
                    }
                }
            }
        }
        i += 1;
    }
    if inv.cmd.is_empty() {
        inv.cmd = "settings".into();
    }
    Ok(inv)
}

fn parse_event(s: &str) -> Result<EventKind, String> {
    if let Some(k) = EventKind::from_id(s) {
        return Ok(k);
    }
    let probe = s.trim().trim_end_matches(".mp3").trim_end_matches(".m4a");
    for d in EVENTS {
        if d.title_zh == s || d.title_en == s || d.stem() == probe || d.kind.id() == probe {
            return Ok(d.kind);
        }
    }
    Err(format!(
        "不认识的事件「{s}」，可用：{}",
        EVENTS
            .iter()
            .map(|d| d.kind.id())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

pub fn load_config(paths: &eds_core::Paths) -> Result<(Config, bool), String> {
    let (mut cfg, fresh) = match Config::load(&paths.config) {
        Ok(Some(c)) => (c, false),
        Ok(None) => (Config::factory(), true),
        Err(e) => return Err(e.to_string()),
    };
    // A downloaded archive already contains its sound pack; adopt it instead of
    // starting mute and making the user point at their own folder.
    if cfg.sounds.dir.is_empty() {
        if let Some(dir) = paths::shipped_sounds() {
            cfg.sounds.dir = dir.display().to_string();
            let _ = cfg.save(&paths.config);
        }
    }
    Ok((cfg, fresh))
}

pub fn maybe_set_sounds_dir(inv: &Invocation, cfg: &mut Config, paths: &eds_core::Paths) -> Result<(), String> {
    if let Some(dir) = &inv.sounds {
        let checked = resolve::validate_sound_dir(dir, cfg.sounds.allow_network_share)?;
        cfg.sounds.dir = checked.display().to_string();
        cfg.save(&paths.config).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let inv = match parse_args() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("edscene: {e}\n\n{HELP}");
            return ExitCode::FAILURE;
        }
    };
    match run(inv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("edscene: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(inv: Invocation) -> Result<(), String> {
    match inv.cmd.as_str() {
        "help" => {
            print!("{HELP}");
            Ok(())
        }
        "version" => {
            println!("edscene {VERSION} ({TARGET})");
            Ok(())
        }
        "config-path" => config_path(&inv),
        "selftest" => selftest(&inv),
        "simulate" => simulate(&inv),
        "signals" => signals(&inv),
        "factory-reset" => factory_reset(&inv),
        "toggle" => toggle(&inv),
        "settings" => settings(&inv),
        "autostart-on" | "autostart-off" => set_autostart_cli(inv.cmd.ends_with("-on")),
        "watch" | "agent" | "service" | "install-service" | "uninstall-service" | "service-status" => {
            platform(&inv)
        }
        other => Err(format!("未实现的命令 {other}")),
    }
}

fn config_path(inv: &Invocation) -> Result<(), String> {
    let p = paths::active_paths();
    if inv.json {
        println!(
            "{}",
            serde_json::json!({
                "config": p.config.display().to_string(),
                "override_dir": p.override_dir.display().to_string(),
                "state_dir": p.state_dir.display().to_string(),
                "events_log": log::file(&p.state_dir).display().to_string(),
                "target": TARGET,
            })
        );
    } else {
        println!("配置      {}", p.config.display());
        println!("替换目录  {}", p.override_dir.display());
        println!("状态目录  {}", p.state_dir.display());
        println!("事件日志  {}", log::file(&p.state_dir).display());
    }
    Ok(())
}

/// The GUI switch and the CLI have to do exactly the same thing, so each
/// platform has one implementation of "autostart" and no second truth.
fn set_autostart_cli(on: bool) -> Result<(), String> {
    crate::platform::set_autostart(on)?;
    println!(
        "开机自启 → {}（{}）",
        if on { "开" } else { "关" },
        if cfg!(windows) { "服务启动类型" } else { "systemd 用户单元 + XDG autostart" }
    );
    Ok(())
}

fn settings(inv: &Invocation) -> Result<(), String> {
    let paths = paths::active_paths();
    let (mut cfg, _) = load_config(&paths)?;
    maybe_set_sounds_dir(inv, &mut cfg, &paths)?;
    ui::show(cfg, paths, inv.hidden)
}

fn selftest(inv: &Invocation) -> Result<(), String> {
    let paths = paths::active_paths();
    let (mut cfg, fresh) = load_config(&paths)?;
    maybe_set_sounds_dir(inv, &mut cfg, &paths)?;
    let rs = resolve::resolve_all(&cfg, &paths);

    if let Some(spec) = &inv.event {
        let kind = parse_event(spec)?;
        let r = rs.iter().find(|r| r.kind == kind).expect("resolve_all covers every kind");
        let path = match (&inv.file, &r.path) {
            (Some(p), _) => p.clone(),
            (None, Some(p)) => p.clone(),
            (None, None) => {
                return Err(format!(
                    "{}：找不到素材（期望 {}）。先运行 --sounds <目录>",
                    def(kind).title_zh,
                    r.preset_file
                ))
            }
        };
        let volume = cfg.volume_for(kind);
        if !inv.quiet {
            println!("试听 {} ← {}（音量 {volume:.2}）", def(kind).title_zh, path.display());
        }
        let info = player::play_blocking(&path, volume, std::time::Duration::from_secs(30))
            .map_err(|e| e.to_string())?;
        if inv.json {
            println!(
                "{}",
                serde_json::json!({"id": kind.id(), "file": path.display().to_string(),
                                   "device": info.device, "source": info.file})
            );
        } else {
            println!("完成 输出={} 素材={}", info.device, info.file);
        }
        return Ok(());
    }

    let mut broken = Vec::new();
    if inv.json {
        let rows: Vec<_> = rs
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.kind.id(),
                    "title": def(r.kind).title_zh,
                    "enabled": cfg.is_enabled(r.kind),
                    "preset": r.preset_file,
                    "status": r.status.label(),
                    "ok": r.status.is_ok(),
                    "source": r.source.map(|s| s.label()),
                    "container": r.container.label(),
                    "path": r.path.as_ref().map(|p| p.display().to_string()),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({"config": paths.config.display().to_string(), "fresh": fresh, "events": rows})
        );
    } else {
        println!(
            "{:<22}{:<6}{:<10}{:<12}{}",
            "事件", "启用", "来源", "容器", "状态 / 文件"
        );
        for r in &rs {
            let d = def(r.kind);
            let tail = match &r.path {
                Some(p) => format!(
                    "{}  {}",
                    r.status.label(),
                    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                ),
                None => format!("{}  (期望 {})", r.status.label(), d.preset_file),
            };
            println!(
                "{:<22}{:<6}{:<10}{:<12}{}",
                d.title_zh,
                if cfg.is_enabled(r.kind) { "开" } else { "关" },
                r.source.map(|s| s.label()).unwrap_or("—"),
                r.container.label(),
                tail
            );
            if cfg.is_enabled(r.kind) && !r.status.is_ok() {
                broken.push(d.title_zh);
            }
        }
        if cfg.sounds.dir.trim().is_empty() {
            println!("\n还没有素材目录：用 --sounds <目录> 指向你的播报音频文件夹。");
        }
        let (ok, total) = resolve::readiness(&cfg, &paths);
        println!("\n就绪 {ok}/{total} 个已启用的播报。");
    }
    if !broken.is_empty() {
        return Err(format!("已启用但找不到素材：{}", broken.join("、")));
    }
    Ok(())
}

fn simulate(inv: &Invocation) -> Result<(), String> {
    let spec = inv.event.as_deref().ok_or("--simulate 需要至少一个事件")?;
    let kinds: Vec<EventKind> = spec
        .split(',')
        .map(|s| parse_event(s.trim()))
        .collect::<Result<_, _>>()?;
    let paths = paths::active_paths();
    let (cfg, _) = load_config(&paths)?;
    let out = Arc::new(LocalBroadcast::new(cfg.clone(), paths.clone()));
    let pipe = Pipeline::new(&cfg, out.clone());

    // Fail fast on material that cannot play, before making any noise.
    for k in &kinds {
        let r = out.resolve(*k);
        if r.status != SoundStatus::Ready {
            return Err(format!(
                "{} 的素材不可用：{}（期望 {}）",
                def(*k).title_zh,
                r.status.label(),
                r.preset_file
            ));
        }
    }
    let mut at = log::mono_ms();
    for k in kinds {
        if !inv.quiet {
            let note = if cfg.is_enabled(k) { "" } else { "  （该播报已关闭，只解析不出声）" };
            println!("→ {} ({}){note}", def(k).title_zh, k.id());
        }
        pipe.force(k, at);
        let gap = inv.interval_ms.max(cfg.event(k).min_interval_ms);
        std::thread::sleep(std::time::Duration::from_millis(gap));
        at += gap;
    }
    out.engine().wait_idle(std::time::Duration::from_secs(30));
    let (dropped, live) = out.engine().stats();
    if !inv.quiet {
        println!("完成。输出流已释放 {dropped} 次，当前仍占用设备 = {live}");
    }
    Ok(())
}

/// Replay a signal script through the real state machine. With `--quiet` no
/// audio is opened at all, which is what CI uses.
fn signals(inv: &Invocation) -> Result<(), String> {
    let path = inv.script.as_ref().ok_or("--signals 需要一个脚本文件")?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path:?}: {e}"))?;
    let paths = paths::active_paths();
    let (cfg, _) = load_config(&paths)?;
    let out: Arc<dyn Broadcast> = if inv.quiet {
        Arc::new(NullBroadcast)
    } else {
        Arc::new(LocalBroadcast::new(cfg.clone(), paths.clone()))
    };
    let mut d = Dispatcher::new(cfg.dispatch_config(), 0);
    let mut fired = 0usize;
    let mut seq = 0u64;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (sig, at) = parse_signal(line).map_err(|e| format!("第 {} 行: {e}", n + 1))?;
        if !inv.quiet {
            println!("[{at:>9} ms] {}", describe_signal(&sig));
        }
        for k in d.apply(sig, at) {
            fired += 1;
            seq += 1;
            if !inv.quiet {
                println!("            ↳ 播报 {}", def(k).title_zh);
            }
            out.fire(k, seq, at);
        }
    }
    out.wait_idle(std::time::Duration::from_secs(30));
    println!("共触发 {fired} 次播报");
    if fired == 0 {
        return Err("脚本没有产生任何播报，检查信号名与阈值".into());
    }
    Ok(())
}

fn parse_signal(line: &str) -> Result<(Signal, u64), String> {
    let v: serde_json::Value = serde_json::from_str(line).map_err(|e| format!("不是 JSON: {e}"))?;
    let at = v.get("at").and_then(|x| x.as_u64()).unwrap_or(0);
    let kind = v
        .get("signal")
        .and_then(|x| x.as_str())
        .ok_or(r#"需要 {"signal": "..."}"#)?;
    let session = v.get("session").and_then(|x| x.as_u64()).unwrap_or(1) as u32;
    let sig = match kind {
        "logon" => Signal::Logon { session },
        "unlock" => Signal::Unlock { session },
        "lock" => Signal::Lock { session },
        "logoff" => Signal::Logoff { session },
        "console_connect" => Signal::ConsoleConnect { session },
        "console_disconnect" => Signal::ConsoleDisconnect,
        "display_on" => Signal::DisplayOn,
        "display_off" => Signal::DisplayOff,
        "suspending" => Signal::Suspending,
        "resuming" => Signal::Resuming,
        "power" => Signal::Power {
            on_ac: v.get("on_ac").and_then(|x| x.as_bool()).unwrap_or(true),
            percent: v
                .get("percent")
                .and_then(|x| x.as_u64())
                .unwrap_or(100)
                .min(255) as u8,
            saver: v.get("saver").and_then(|x| x.as_bool()),
        },
        other => return Err(format!("未知信号 {other}")),
    };
    Ok((sig, at))
}

fn describe_signal(s: &Signal) -> String {
    match s {
        Signal::Logon { session } => format!("logon(session={session})"),
        Signal::Unlock { session } => format!("unlock(session={session})"),
        Signal::Lock { session } => format!("lock(session={session})"),
        Signal::Logoff { session } => format!("logoff(session={session})"),
        Signal::ConsoleConnect { session } => format!("console_connect(session={session})"),
        Signal::ConsoleDisconnect => "console_disconnect".into(),
        Signal::DisplayOn => "display_on".into(),
        Signal::DisplayOff => "display_off".into(),
        Signal::Suspending => "suspending".into(),
        Signal::Resuming => "resuming".into(),
        Signal::Power {
            on_ac,
            percent,
            saver,
        } => format!("power(ac={on_ac} pct={percent} saver={saver:?})"),
    }
}

fn factory_reset(inv: &Invocation) -> Result<(), String> {
    let paths = paths::active_paths();
    let retired = resolve::retire_all_overrides(&paths).map_err(|e| e.to_string())?;
    let cfg = Config::factory();
    cfg.save(&paths.config).map_err(|e| e.to_string())?;
    if inv.json {
        println!(
            "{}",
            serde_json::json!({"reset": true, "retired_overrides": retired,
                               "config": paths.config.display().to_string()})
        );
    } else {
        println!("已恢复出厂设置；{retired} 个替换素材退回 .disabled（未删除任何文件）。");
        println!("配置 {}", paths.config.display());
    }
    Ok(())
}

/// Scriptable switch, used by the docs and by headless verification.
fn toggle(inv: &Invocation) -> Result<(), String> {
    let spec = inv.event.as_deref().ok_or("--enable/--disable 需要事件名")?;
    let kind = parse_event(spec)?;
    let paths = paths::active_paths();
    let (mut cfg, _) = load_config(&paths)?;
    cfg.event_mut(kind).enabled = inv.on;
    cfg.save(&paths.config).map_err(|e| e.to_string())?;
    println!(
        "{} {} → {}",
        def(kind).title_zh,
        kind.id(),
        if inv.on { "开" } else { "关" }
    );
    Ok(())
}

#[cfg(windows)]
fn platform(inv: &Invocation) -> Result<(), String> {
    match inv.cmd.as_str() {
        "watch" => win::watch(inv),
        "agent" => win::agent(inv),
        "service-status" => win::service::status(),
        _ => win::service(inv),
    }
}

#[cfg(not(windows))]
fn platform(inv: &Invocation) -> Result<(), String> {
    match inv.cmd.as_str() {
        "watch" => linux::watch(inv),
        "agent" => linux::agent(inv),
        other => Err(format!("Linux 没有服务概念；请用 systemd 用户单元（--config-path 查看路径），未知命令 {other}")),
    }
}

#[cfg(windows)]
mod win;
#[cfg(not(windows))]
mod linux;
