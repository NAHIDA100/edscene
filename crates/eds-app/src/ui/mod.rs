//! Settings window: Slint UI on top of the same config/pipeline the CLI uses.
//!
//! The Rust side owns no rules — it reads and writes [`Config`], re-resolves
//! the sound pack, and asks the platform layer for the things only it can do
//! (file dialogs, service registration, desktop capture).

use eds_core::config::{BlurMode, Config, Speed, Theme};
use eds_core::presets::{def, EVENTS};
use eds_core::resolve::{self, SoundStatus};
use eds_core::{EventKind, Paths};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

slint::include_modules!();

/// 谷地黄 from the reference shot; 武陵蓝 is the user-supplied #35A8BB.
const ACCENTS: [(u32, u32); 2] = [(0xF5FF01, 0x2A2C2E), (0x35A8BB, 0x08252A)];

struct State {
    cfg: RefCell<Config>,
    paths: Paths,
    reset_armed: Cell<bool>,
    blur_timer: RefCell<Option<Timer>>,
}

pub fn show(cfg: Config, paths: Paths) -> Result<(), String> {
    let win = MainWindow::new().map_err(|e| format!("无法创建窗口：{e}"))?;
    let st = Rc::new(State {
        cfg: RefCell::new(cfg),
        paths,
        reset_armed: Cell::new(false),
        blur_timer: RefCell::new(None),
    });
    let weak = win.as_weak();
    st.paint(&win);
    win.set_version(SharedString::from(format!(
        "{} {}",
        env!("CARGO_PKG_VERSION"),
        env!("EDSCENE_TARGET")
    )));
    win.set_config_path(SharedString::from(st.paths.config.display().to_string()));

    let s = st.clone();
    let w = weak.clone();
    win.on_toggle_row(move |idx, on| {
        let Some(win) = w.upgrade() else { return };
        let Some(kind) = kind_at(idx) else { return };
        s.cfg.borrow_mut().event_mut(kind).enabled = on;
        s.commit(&win, format!("{} → {}", def(kind).title_zh, if on { "开" } else { "关" }));
    });

    let s = st.clone();
    let w = weak.clone();
    win.on_preview(move |idx| {
        let Some(win) = w.upgrade() else { return };
        let Some(kind) = kind_at(idx) else { return };
        let r = resolve::resolve(&s.cfg.borrow(), &s.paths, kind);
        let Some(path) = r.path else {
            win.set_status_text(SharedString::from(format!(
                "！{} 找不到素材（{}）",
                def(kind).title_zh, r.preset_file
            )));
            return;
        };
        if r.status != SoundStatus::Ready {
            win.set_status_text(SharedString::from(format!(
                "！{}：{}",
                def(kind).title_zh, r.status.label()
            )));
            return;
        }
        let volume = s.cfg.borrow().volume_for(kind);
        win.set_status_text(SharedString::from(format!("试听 {}…", def(kind).title_zh)));
        std::thread::spawn(move || {
            let _ = crate::player::play_blocking(&path, volume, Duration::from_secs(30));
        });
    });

    let s = st.clone();
    let w = weak.clone();
    win.on_replace(move |idx| {
        let Some(win) = w.upgrade() else { return };
        let Some(kind) = kind_at(idx) else { return };
        let Some(picked) = pick_file_pub("选择替代音频") else { return };
        let copy = !s.cfg.borrow().sounds.link_instead_of_copy;
        match resolve::install_override(&s.paths, kind, &picked, copy) {
            Ok(dst) => {
                s.cfg.borrow_mut().event_mut(kind).file = String::new();
                s.commit(&win, format!("{} 已替换为 {}", def(kind).title_zh, dst.display()));
            }
            Err(e) => win.set_status_text(SharedString::from(format!("！替换失败：{e}"))),
        }
    });

    let s = st.clone();
    let w = weak.clone();
    win.on_restore_one(move |idx| {
        let Some(win) = w.upgrade() else { return };
        let Some(kind) = kind_at(idx) else { return };
        let n = resolve::retire_overrides(&s.paths, kind).unwrap_or(0);
        s.cfg.borrow_mut().event_mut(kind).file = String::new();
        s.commit(&win, format!("{} 已回到预设（退回 {n} 个替换文件）", def(kind).title_zh));
    });

    let s = st.clone();
    let w = weak.clone();
    win.on_reset_defaults(move || {
        let Some(win) = w.upgrade() else { return };
        if !s.reset_armed.replace(true) {
            win.set_status_text(SharedString::from("再点一次「恢复默认」确认：所有设置回到出厂状态"));
            let s2 = s.clone();
            let w2 = w.clone();
            let t = Timer::default();
            let _ = t.start(TimerMode::SingleShot, Duration::from_secs(6), move || {
                s2.reset_armed.set(false);
                if let Some(win) = w2.upgrade() {
                    win.set_status_text(SharedString::from("已取消"));
                }
            });
            return;
        }
        s.reset_armed.set(false);
        let retired = resolve::retire_all_overrides(&s.paths).unwrap_or(0);
        *s.cfg.borrow_mut() = Config::factory();
        s.commit(&win, format!("已恢复出厂设置（退回 {retired} 个替换文件）"));
    });

    let s = st.clone();
    let w = weak.clone();
    win.on_pick_sounds(move || {
        let Some(win) = w.upgrade() else { return };
        let Some(dir) = pick_folder_pub("选择播报音频所在文件夹") else { return };
        let allow = s.cfg.borrow().sounds.allow_network_share;
        match resolve::validate_sound_dir(&dir.display().to_string(), allow) {
            Ok(p) => {
                s.cfg.borrow_mut().sounds.dir = p.display().to_string();
                s.commit(&win, format!("素材目录 → {}", p.display()));
            }
            Err(e) => win.set_status_text(SharedString::from(format!("！{e}"))),
        }
    });

    let s = st.clone();
    win.on_open_sounds_dir(move || {
        let dir = s.cfg.borrow().sounds.dir.clone();
        let p = if dir.is_empty() { s.paths.override_dir.clone() } else { std::path::PathBuf::from(dir) };
        let _ = std::fs::create_dir_all(&p);
        open_dir_pub(&p);
    });

    let s = st.clone();
    let w = weak.clone();
    win.on_set_accent(move |i| {
        let Some(win) = w.upgrade() else { return };
        s.cfg.borrow_mut().ui.theme = if i == 1 { Theme::WulingBlue } else { Theme::ValleyYellow };
        s.commit(&win, "强调色已更新".into());
    });
    let s = st.clone();
    let w = weak.clone();
    win.on_set_blur(move |i| {
        let Some(win) = w.upgrade() else { return };
        s.cfg.borrow_mut().ui.blur_mode = match i {
            0 => BlurMode::Realtime,
            1 => BlurMode::WallpaperCopy,
            _ => BlurMode::Solid,
        };
        s.apply_backdrop(&win);
        s.commit(&win, "背景模式已更新".into());
    });
    let s = st.clone();
    let w = weak.clone();
    win.on_set_speed(move |i| {
        let Some(win) = w.upgrade() else { return };
        s.cfg.borrow_mut().ui.animation = match i {
            0 => Speed::Slow,
            1 => Speed::Standard,
            2 => Speed::Fast,
            _ => Speed::Off,
        };
        s.paint_palette(&win);
        s.commit(&win, "动效已更新".into());
    });
    let s = st.clone();
    let w = weak.clone();
    win.on_set_deco(move |i| {
        let Some(win) = w.upgrade() else { return };
        s.cfg.borrow_mut().ui.decoration = i.clamp(0, 3) as u8;
        s.paint_palette(&win);
        s.commit(&win, "装饰强度已更新".into());
    });
    let s = st.clone();
    let w = weak.clone();
    win.on_set_top_bar_dim(move |on| {
        let Some(win) = w.upgrade() else { return };
        s.cfg.borrow_mut().ui.top_bar_dim = on;
        s.commit(&win, format!("顶栏压暗 → {}", if on { "开" } else { "关" }));
    });
    let s = st.clone();
    let w = weak.clone();
    win.on_set_rounded(move |on| {
        let Some(win) = w.upgrade() else { return };
        s.cfg.borrow_mut().ui.corner_rounded = on;
        s.paint_palette(&win);
        s.commit(&win, format!("圆角 → {}", if on { "开" } else { "关" }));
    });
    let s = st.clone();
    let w = weak.clone();
    win.on_set_volume(move |v| {
        let Some(win) = w.upgrade() else { return };
        let pct = (v * 100.0).round().clamp(0.0, 100.0) as u8;
        s.cfg.borrow_mut().audio.volume = pct;
        win.set_volume_text(SharedString::from(format!("{pct}%")));
        s.save();
        s.refresh_rows(&win);
    });
    let s = st.clone();
    let w = weak.clone();
    win.on_set_autostart(move |on| {
        let Some(win) = w.upgrade() else { return };
        match crate::platform::set_autostart(on) {
            Ok(()) => {
                s.cfg.borrow_mut().runtime.autostart = on;
                s.commit(&win, format!("开机自启 → {}", if on { "开" } else { "关" }));
            }
            Err(e) => win.set_status_text(SharedString::from(format!("！{e}"))),
        }
    });
    let s = st.clone();
    let w = weak.clone();
    win.on_set_prelogon(move |on| {
        let Some(win) = w.upgrade() else { return };
        s.cfg.borrow_mut().runtime.experimental_prelogon = on;
        s.commit(
            &win,
            format!(
                "登录界面播报 → {}（需要服务已注册）",
                if on { "开" } else { "关" }
            ),
        );
    });
    let w = weak.clone();
    win.on_close_window(move || {
        if let Some(win) = w.upgrade() {
            win.hide().ok();
        }
        slint::quit_event_loop().ok();
    });

    win.show().map_err(|e| format!("无法显示窗口：{e}"))?;
    st.apply_backdrop(&win);
    slint::run_event_loop().map_err(|e| e.to_string())?;
    // Remember the open tab for next time.
    let tab = win.get_tab().clamp(0, 2) as u8;
    let mut cfg = st.cfg.borrow_mut();
    if cfg.ui.tab != tab {
        cfg.ui.tab = tab;
        let _ = cfg.save(&st.paths.config);
    }
    Ok(())
}

fn kind_at(idx: i32) -> Option<EventKind> {
    EVENTS.get(idx as usize).map(|d| d.kind)
}

impl State {
    /// Push config → window: palette, options, row model.
    fn paint(&self, win: &MainWindow) {
        let cfg = self.cfg.borrow();
        self.paint_palette(win);
        win.set_accent_index(match cfg.ui.theme {
            Theme::ValleyYellow => 0,
            Theme::WulingBlue => 1,
        });
        win.set_blur_index(match cfg.ui.blur_mode {
            BlurMode::Realtime => 0,
            BlurMode::WallpaperCopy => 1,
            BlurMode::Solid => 2,
        });
        win.set_speed_index(match cfg.ui.animation {
            Speed::Slow => 0,
            Speed::Standard => 1,
            Speed::Fast => 2,
            Speed::Off => 3,
        });
        win.set_deco_index(cfg.ui.decoration as i32);
        win.set_top_bar_dim(cfg.ui.top_bar_dim);
        win.set_tab(cfg.ui.tab as i32);
        win.set_rounded(cfg.ui.corner_rounded);
        win.set_autostart(cfg.runtime.autostart);
        win.set_prelogon(cfg.runtime.experimental_prelogon);
        win.set_sounds_dir(SharedString::from(cfg.sounds.dir.clone()));
        win.set_volume(cfg.volume_for_display());
        win.set_volume_text(SharedString::from(format!("{}%", cfg.audio.volume)));
        win.set_service_ready(crate::platform::service_installed());
        drop(cfg);
        self.refresh_rows(win);
    }

    fn paint_palette(&self, win: &MainWindow) {
        let cfg = self.cfg.borrow();
        let (accent, ink) = match cfg.ui.theme {
            Theme::ValleyYellow => ACCENTS[0],
            Theme::WulingBlue => ACCENTS[1],
        };
        let pal = win.global::<Palette>();
        pal.set_accent(rgb(accent));
        pal.set_accent_ink(rgb(ink));
        let dur = match cfg.ui.animation {
            Speed::Slow => 320,
            Speed::Standard => 180,
            Speed::Fast => 90,
            Speed::Off => 0,
        };
        pal.set_anim(dur);
        pal.set_hatch(cfg.ui.decoration > 0);
        pal.set_rounded(cfg.ui.corner_rounded);
        let radius = match cfg.ui.decoration {
            0 => 6.0,
            1 => 8.0,
            3 => 16.0,
            _ => 10.0,
        };
        pal.set_radius(radius);
    }

    fn refresh_rows(&self, win: &MainWindow) {
        let cfg = self.cfg.borrow();
        let zh = cfg.ui.language == eds_core::config::Language::Zh;
        let rows: Vec<EventRow> = EVENTS
            .iter()
            .map(|d| {
                let r = resolve::resolve(&cfg, &self.paths, d.kind);
                EventRow {
                    id: SharedString::from(d.kind.id()),
                    title: SharedString::from(if zh { d.title_zh } else { d.title_en }),
                    hint: SharedString::from(if zh { d.hint_zh } else { d.hint_en }),
                    enabled: cfg.is_enabled(d.kind),
                    status: SharedString::from(r.status.label()),
                    ok: r.status.is_ok(),
                    source: SharedString::from(r.source.map(|s| s.label()).unwrap_or("—")),
                    file: SharedString::from(r.file_name()),
                    container: SharedString::from(r.container.label()),
                }
            })
            .collect();
        win.set_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    }

    fn save(&self) {
        let cfg = self.cfg.borrow();
        if let Err(e) = cfg.save(&self.paths.config) {
            eprintln!("edscene: 写入配置失败 {e}");
        }
    }

    fn commit(&self, win: &MainWindow, msg: String) {
        self.save();
        let cfg = self.cfg.borrow();
        let (ok, total) = resolve::readiness(&cfg, &self.paths);
        win.set_status_text(SharedString::from(format!("{msg} · 已提交（就绪 {ok}/{total}）")));
        drop(cfg);
        self.refresh_rows(win);
        let cfg = self.cfg.borrow();
        win.set_sounds_dir(SharedString::from(cfg.sounds.dir.clone()));
    }

    /// 纯色 / 壁纸副本 / 实时模糊. Capture failures degrade to 纯色 quietly,
    /// because a missing compositor is normal on a server or inside a VM.
    fn apply_backdrop(&self, win: &MainWindow) {
        let mode = self.cfg.borrow().ui.blur_mode;
        let mut timer = self.blur_timer.borrow_mut();
        *timer = None;
        match mode {
            BlurMode::Solid => {
                win.set_has_wallpaper(false);
            }
            BlurMode::WallpaperCopy => match crate::platform::blurred_wallpaper() {
                Some(img) => {
                    win.set_wallpaper(img);
                    win.set_has_wallpaper(true);
                }
                None => win.set_has_wallpaper(false),
            },
            BlurMode::Realtime => {
                if let Some(img) = crate::platform::blurred_wallpaper() {
                    win.set_wallpaper(img);
                    win.set_has_wallpaper(true);
                }
                let weak = win.as_weak();
                let t = Timer::default();
                let _ = t.start(TimerMode::Repeated, Duration::from_millis(2000), move || {
                    let Some(win) = weak.upgrade() else { return };
                    if let Some(img) = crate::platform::blurred_wallpaper() {
                        win.set_wallpaper(img);
                    }
                });
                *timer = Some(t);
            }
        }
    }
}

/// `v` is 0xRRGGBB.
fn rgb(v: u32) -> slint::Color {
    slint::Color::from_argb_u8(
        255,
        ((v >> 16) & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        (v & 0xff) as u8,
    )
}

/// Keep the platform calls in one place so the Linux build links nothing extra.
#[cfg(windows)]
fn pick_file_pub(title: &str) -> Option<std::path::PathBuf> {
    crate::win::dialogs::pick_file(title)
}
#[cfg(windows)]
fn pick_folder_pub(title: &str) -> Option<std::path::PathBuf> {
    crate::win::dialogs::pick_folder(title)
}
#[cfg(windows)]
fn open_dir_pub(p: &std::path::Path) {
    crate::win::dialogs::reveal(p)
}
#[cfg(not(windows))]
fn pick_file_pub(_title: &str) -> Option<std::path::PathBuf> {
    crate::linux::pick_file()
}
#[cfg(not(windows))]
fn pick_folder_pub(_title: &str) -> Option<std::path::PathBuf> {
    crate::linux::pick_folder()
}
#[cfg(not(windows))]
fn open_dir_pub(p: &std::path::Path) {
    crate::linux::open_dir(p)
}
