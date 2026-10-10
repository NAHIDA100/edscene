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
    status_timer: RefCell<Option<Timer>>,
    grab: RefCell<Option<crate::platform::drag::Grab>>,
}

impl State {
    /// The banner at the bottom is the only feedback the window has, so make it
    /// say what happened, in what colour, and then get out of the way.
    fn status(&self, win: &MainWindow, msg: String, bad: bool) {
        win.set_status_text(SharedString::from(msg));
        win.set_status_bad(bad);
        let weak = win.as_weak();
        let mut t = self.status_timer.borrow_mut();
        let timer = Timer::default();
        let _ = timer.start(TimerMode::SingleShot, Duration::from_secs(8), move || {
            if let Some(win) = weak.upgrade() {
                win.set_status_text(SharedString::from(""));
            }
        });
        *t = Some(timer);
    }
}

pub fn show(cfg: Config, paths: Paths, hidden: bool) -> Result<(), String> {
    let win = MainWindow::new().map_err(|e| format!("无法创建窗口：{e}"))?;
    let st = Rc::new(State {
        cfg: RefCell::new(cfg),
        paths,
        reset_armed: Cell::new(false),
        blur_timer: RefCell::new(None),
        status_timer: RefCell::new(None),
        grab: RefCell::new(None),
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
            s.status(&win, format!("！{} 找不到素材（{}）", def(kind).title_zh, r.preset_file), true);
            return;
        };
        if r.status != SoundStatus::Ready {
            s.status(&win, format!("！{}：{}", def(kind).title_zh, r.status.label()), true);
            return;
        }
        let volume = s.cfg.borrow().volume_for(kind);
        s.status(&win, format!("试听 {}…", def(kind).title_zh), false);
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
            Err(e) => s.status(&win, format!("！替换失败：{e}"), true),
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
            s.status(&win, "再点一次「恢复默认」确认：所有设置回到出厂状态".into(), false);
            let s2 = s.clone();
            let w2 = w.clone();
            let t = Timer::default();
            let _ = t.start(TimerMode::SingleShot, Duration::from_secs(6), move || {
                s2.reset_armed.set(false);
                if let Some(win) = w2.upgrade() {
                    s2.status(&win, "已取消".into(), false);
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
            Err(e) => s.status(&win, format!("！{e}"), true),
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
    win.on_set_frame(move |i| {
        let Some(win) = w.upgrade() else { return };
        s.cfg.borrow_mut().ui.native_frame = i == 1;
        // 换边框要重建窗口样式，Slint 在下一次显示时才会应用；先把属性推过去。
        s.sync(&win);
        s.commit(&win, format!("窗口边框 → {}", if i == 1 { "系统标题栏" } else { "无边框" }));
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
                win.set_need_admin(false);
                s.commit(&win, format!("开机自启 → {}", if on { "开" } else { "关" }));
            }
            // A bare sentence in the corner is not something you can act on.
            Err(e) => {
                win.set_need_admin(true);
                s.status(&win, format!("！{e}"), true);
            }
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
    let s = st.clone();
    let w = weak.clone();
    win.on_pick_backdrop(move || {
        let Some(win) = w.upgrade() else { return };
        let Some(picked) = pick_file_pub("选择背景图片") else { return };
        let shown = picked.display().to_string();
        s.cfg.borrow_mut().ui.backdrop = shown.clone();
        s.apply_backdrop(&win);
        s.commit(&win, format!("背景图片 → {}", file_name(&shown)));
    });
    let s = st.clone();
    let w = weak.clone();
    win.on_clear_backdrop(move || {
        let Some(win) = w.upgrade() else { return };
        s.cfg.borrow_mut().ui.backdrop = String::new();
        s.apply_backdrop(&win);
        s.commit(&win, "背景图片 → 跟随系统桌面壁纸".into());
    });
    let s = st.clone();
    let w = weak.clone();
    win.on_elevate(move || {
        let Some(win) = w.upgrade() else { return };
        match crate::platform::elevate_for_autostart() {
            Ok(()) => s.status(&win, "已请求管理员权限，请在弹窗里点「是」".into(), false),
            Err(e) => s.status(&win, format!("！{e}"), true),
        }
    });
    let s = st.clone();
    let w = weak.clone();
    win.on_drag_start(move || {
        let Some(win) = w.upgrade() else { return };
        let _ = &win;
        // Every step lands in the log, because "拖不动" has at least three
        // different causes and the user cannot tell them apart.
        match crate::platform::drag::begin() {
            Ok(g) => {
                crate::log::write(
                    &s.paths.state_dir,
                    &serde_json::json!({"ev": "drag", "step": "begin", "what": g.describe()}),
                );
                match crate::platform::drag::modal(&g) {
                    Ok(()) => crate::log::write(
                        &s.paths.state_dir,
                        &serde_json::json!({"ev": "drag", "step": "modal-ok"}),
                    ),
                    Err(e) => {
                        // No window manager help: move it ourselves per event.
                        crate::log::write(
                            &s.paths.state_dir,
                            &serde_json::json!({"ev": "drag", "step": "modal-fail", "err": e}),
                        );
                        *s.grab.borrow_mut() = Some(g);
                    }
                }
            }
            Err(e) => crate::log::write(
                &s.paths.state_dir,
                &serde_json::json!({"ev": "drag", "step": "begin-fail", "err": e}),
            ),
        }
    });
    let s = st.clone();
    win.on_drag_move(move || {
        if let Some(g) = s.grab.borrow().as_ref() {
            crate::platform::drag::apply(g);
        }
    });
    let s = st.clone();
    win.on_drag_end(move || {
        *s.grab.borrow_mut() = None;
    });
    // The tray is what makes "close" mean "put it away" instead of "shut it
    // down". Desktops without a notification area just keep the old behaviour.
    let tray = Tray::new().ok();
    let has_tray = tray.is_some();
    if let Some(tray) = tray.as_ref() {
        let w = weak.clone();
        tray.on_open(move || {
            if let Some(win) = w.upgrade() {
                win.show().ok();
            }
        });
        let t = tray.as_weak();
        tray.on_quit(move || {
            // Dropping the component at the end of `show` is what unhooks the
            // icon; the weak upgrade only keeps this closure honest.
            if t.upgrade().is_some() {
                slint::quit_event_loop().ok();
            }
        });
    }
    let s = st.clone();
    let w = weak.clone();
    win.on_set_tray(move |on| {
        let Some(win) = w.upgrade() else { return };
        s.cfg.borrow_mut().ui.tray = on;
        win.set_tray_on(on);
        s.commit(
            &win,
            format!(
                "托盘常驻 → {}（{}）",
                if on { "开" } else { "关" },
                if on { "下次登录起服务会带上设置窗口" } else { "本次退出后不再常驻" }
            ),
        );
    });
    // 系统标题栏的 × 也要"收进托盘"而不是销毁窗口：藏起来并告诉 Slint 别关，
    // 不然托盘的「打开设置」就唤不回它了。
    {
        let w = weak.clone();
        win.window().on_close_requested(move || {
            if let Some(win) = w.upgrade() {
                win.hide().ok();
            }
            if has_tray {
                slint::CloseRequestResponse::KeepWindowShown
            } else {
                slint::quit_event_loop().ok();
                slint::CloseRequestResponse::HideWindow
            }
        });
    }
    let w = weak.clone();
    win.on_close_window(move || {
        if let Some(win) = w.upgrade() {
            win.hide().ok();
        }
        if !has_tray {
            slint::quit_event_loop().ok();
        }
    });

    if !hidden {
        win.show().map_err(|e| format!("无法显示窗口：{e}"))?;
    }
    st.apply_backdrop(&win);
    slint::run_event_loop().map_err(|e| e.to_string())?;
    drop(tray);
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
        self.paint_palette(win);
        // Only at startup: `tab` lives in the window while you click around, and
        // the config gets it back on exit.
        let tab = self.cfg.borrow().ui.tab;
        win.set_tab(tab as i32);
        self.sync(win);
        self.refresh_rows(win);
    }

    /// Every control that shows config state, re-pushed. `commit()` has to call
    /// this: a switch whose property still holds the old value looks like the
    /// click did nothing, which is exactly what users reported.
    fn sync(&self, win: &MainWindow) {
        let cfg = self.cfg.borrow();
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
        win.set_rounded(cfg.ui.corner_rounded);
        win.set_native_frame(cfg.ui.native_frame);
        win.set_autostart(cfg.runtime.autostart);
        win.set_prelogon(cfg.runtime.experimental_prelogon);
        win.set_tray_on(cfg.ui.tray);
        win.set_sounds_dir(SharedString::from(cfg.sounds.dir.clone()));
        win.set_backdrop_name(SharedString::from(file_name(&cfg.ui.backdrop)));
        win.set_volume(cfg.volume_for_display());
        win.set_volume_text(SharedString::from(format!("{}%", cfg.audio.volume)));
        win.set_service_ready(crate::platform::service_installed());
        win.set_native_drag(crate::platform::native_drag(cfg.ui.native_frame));
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
        drop(cfg);
        self.status(win, format!("{msg} · 已提交（就绪 {ok}/{total}）"), false);
        self.sync(win);
        self.refresh_rows(win);
    }

    /// 纯色 / 壁纸副本 / 实时模糊. Capture failures degrade to 纯色 quietly,
    /// because a missing compositor is normal on a server or inside a VM.
    fn apply_backdrop(&self, win: &MainWindow) {
        let cfg = self.cfg.borrow();
        let mode = cfg.ui.blur_mode;
        let picked = cfg.ui.backdrop.clone();
        drop(cfg);
        let mut timer = self.blur_timer.borrow_mut();
        *timer = None;
        match mode {
            BlurMode::Solid => {
                win.set_has_wallpaper(false);
            }
            BlurMode::WallpaperCopy => match crate::platform::backdrop(&picked) {
                Some(img) => {
                    win.set_wallpaper(img);
                    win.set_has_wallpaper(true);
                }
                None => win.set_has_wallpaper(false),
            },
            BlurMode::Realtime => {
                if let Some(img) = crate::platform::backdrop(&picked) {
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

/// Just the last path component, for the 背景图片 row.
fn file_name(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
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
