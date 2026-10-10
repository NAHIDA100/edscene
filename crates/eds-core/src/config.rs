use crate::dispatcher::DispatchConfig;
use crate::event::EventKind;
use crate::presets::EVENTS;
use crate::CONFIG_VERSION;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: String,
        source: std::io::Error,
    },
    #[error("config file is not valid: {0}")]
    Parse(String),
}

/// Where the config lives, plus the app-owned override folder. Computed by the
/// binary (the core stays free of platform calls).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config: std::path::PathBuf,
    pub override_dir: std::path::PathBuf,
    pub state_dir: std::path::PathBuf,
}

impl Paths {
    pub fn disabled_dir(&self) -> std::path::PathBuf {
        self.override_dir.join(".disabled")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    /// Stop whatever is playing and start the new broadcast.
    #[default]
    Interrupt,
    /// Play them one after another.
    Queue,
    /// Ignore the new one while something is playing.
    DropNewest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    ValleyYellow,
    WulingBlue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    Zh,
    En,
}

/// Mirrors the three-state background control in the reference UI. `Solid` is
/// the cheap default and the only one that works without a compositor.
/// The reference UI shows a blurred desktop behind translucent bands, so that
/// is the factory default; `Solid` is the fallback for a session with no
/// compositor or no capturable screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlurMode {
    Realtime,
    #[default]
    WallpaperCopy,
    Solid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Speed {
    Slow,
    #[default]
    Standard,
    Fast,
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sounds {
    /// The user's sound pack, e.g. `D:\works\eds`. Empty until they pick one.
    pub dir: String,
    /// Replace the file in place instead of copying it into the override dir
    /// when the user picks a new clip in the UI.
    pub link_instead_of_copy: bool,
    pub allow_network_share: bool,
}

impl Default for Sounds {
    fn default() -> Self {
        Sounds {
            dir: String::new(),
            link_instead_of_copy: false,
            allow_network_share: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    /// Percent, applied on top of the OS volume. An integer keeps the file
    /// round-trippable instead of drifting through f32.
    pub volume: u8,
    pub policy: Policy,
    pub device: String,
    /// Release the output stream this long after the last broadcast so the
    /// endpoint is not held open by an idle process.
    pub output_idle_ms: u64,
    pub min_interval_ms: u64,
}

impl Default for Audio {
    fn default() -> Self {
        Audio {
            volume: 80,
            policy: Policy::Interrupt,
            device: String::new(),
            output_idle_ms: 5_000,
            min_interval_ms: 1_500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub theme: Theme,
    pub language: Language,
    pub blur_mode: BlurMode,
    pub top_bar_dim: bool,
    pub corner_rounded: bool,
    pub animation: Speed,
    /// React to session events on RDP sessions too (off: virtual audio devices
    /// churn there and the broadcast would land in the wrong place).
    pub include_rdp: bool,
    /// Optional .ttf/.otf used instead of the system CJK fallback chain.
    pub font_file: String,
    /// Backdrop image for the settings window. Empty means "whatever the
    /// desktop is showing"; the shipped default must never be a picture file,
    /// because no artwork is allowed in this repository.
    pub backdrop: String,
    /// Keep a settings window in the notification area, started by the service
    /// at logon, so the GUI is one click away instead of one Explorer trip.
    pub tray: bool,
    /// Draw our own title bar (`false`) or let the window manager decorate the
    /// window (`true`, the default). Dragging, resizing and Win+arrow snapping
    /// are only guaranteed to behave with a real frame.
    pub native_frame: bool,
    /// 0 关闭 … 3 重：controls the diagonal hatch and the corner radius.
    pub decoration: u8,
    /// Last opened tab, so the window comes back where you left it.
    pub tab: u8,
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            theme: Theme::default(),
            language: Language::default(),
            blur_mode: BlurMode::default(),
            top_bar_dim: true,
            corner_rounded: true,
            animation: Speed::default(),
            include_rdp: false,
            font_file: String::new(),
            backdrop: String::new(),
            tray: true,
            native_frame: true,
            decoration: 2,
            tab: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Runtime {
    /// Windows service / systemd unit enabled. The service is the single
    /// source of truth for autostart; the toggle maps to its start type.
    pub autostart: bool,
    pub boot_grace_ms: u64,
    pub power_poll_ms: u64,
    /// An agent with no window quits after this much silence.
    pub idle_exit_ms: u64,
    /// Play at the password prompt by borrowing the winlogon token. Needs the
    /// service and is off because no factory event requires it.
    pub experimental_prelogon: bool,
    pub debug_events: bool,
    pub saver_threshold_percent: u8,
    pub low_battery_rearm_percent: u8,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime {
            autostart: true,
            boot_grace_ms: 90_000,
            power_poll_ms: 2_000,
            idle_exit_ms: 30 * 60 * 1_000,
            experimental_prelogon: false,
            debug_events: false,
            saver_threshold_percent: 20,
            low_battery_rearm_percent: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventCfg {
    pub enabled: bool,
    /// Absolute path picked in the UI; beats the sound directory.
    pub file: String,
    pub min_interval_ms: u64,
    /// Percent, multiplied with the global volume.
    pub volume: u8,
}

impl Default for EventCfg {
    fn default() -> Self {
        EventCfg {
            enabled: false,
            file: String::new(),
            min_interval_ms: 1_500,
            volume: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub sounds: Sounds,
    pub audio: Audio,
    pub ui: Ui,
    pub runtime: Runtime,
    pub events: BTreeMap<String, EventCfg>,
}

impl Default for Config {
    /// The factory state: five broadcasts on, three off, nothing resolved yet.
    fn default() -> Self {
        let mut events = BTreeMap::new();
        for d in EVENTS {
            events.insert(
                d.kind.id().to_string(),
                EventCfg {
                    enabled: d.default_enabled,
                    file: String::new(),
                    min_interval_ms: 1_500,
                    volume: 100,
                },
            );
        }
        Config {
            version: CONFIG_VERSION,
            sounds: Sounds::default(),
            audio: Audio::default(),
            ui: Ui::default(),
            runtime: Runtime::default(),
            events,
        }
    }
}

impl Config {
    pub fn factory() -> Config {
        Config::default()
    }

    pub fn event(&self, kind: EventKind) -> EventCfg {
        self.events
            .get(kind.id())
            .cloned()
            .unwrap_or(EventCfg {
                enabled: crate::presets::def(kind).default_enabled,
                ..Default::default()
            })
    }

    pub fn event_mut(&mut self, kind: EventKind) -> &mut EventCfg {
        self.events
            .entry(kind.id().to_string())
            .or_insert_with(|| EventCfg {
                enabled: crate::presets::def(kind).default_enabled,
                ..Default::default()
            })
    }

    pub fn is_enabled(&self, kind: EventKind) -> bool {
        self.event(kind).enabled
    }

    /// 0..1 for a slider, derived from the integer stored in the file.
    pub fn volume_for_display(&self) -> f32 {
        f32::from(self.audio.volume) / 100.0
    }

    /// Final linear gain for one event: global volume times the event's own.
    pub fn volume_for(&self, kind: EventKind) -> f32 {
        let e = self.event(kind);
        (f32::from(self.audio.volume) * f32::from(e.volume)) / 10_000.0
    }

    /// Events with a switch flipped on, in preset display order.
    pub fn enabled_events(&self) -> Vec<EventKind> {
        EVENTS
            .iter()
            .map(|d| d.kind)
            .filter(|k| self.is_enabled(*k))
            .collect()
    }

    /// Fold the config into the pure state machine's inputs.
    pub fn dispatch_config(&self) -> DispatchConfig {
        let mut cooldowns = BTreeMap::new();
        for d in EVENTS {
            let e = self.event(d.kind);
            let gap = e.min_interval_ms.max(self.audio.min_interval_ms);
            cooldowns.insert(d.kind.id().to_string(), gap);
        }
        DispatchConfig {
            boot_grace_ms: self.runtime.boot_grace_ms,
            low_battery_rearm_percent: self.runtime.low_battery_rearm_percent,
            cooldown_ms: cooldowns,
            saver_threshold_percent: self.runtime.saver_threshold_percent,
        }
    }

    /// Every event the factory table knows about, even ones a hand-edited file
    /// forgot. Unknown keys are left alone so forward-compatible edits survive.
    pub fn normalize(&mut self) {
        for d in EVENTS {
            self.events
                .entry(d.kind.id().to_string())
                .or_insert_with(|| EventCfg {
                    enabled: d.default_enabled,
                    ..Default::default()
                });
        }
        self.version = self.version.max(CONFIG_VERSION);
    }

    /// `to_string_pretty` keeps real `[section]` headers, so the file stays
    /// hand-editable instead of collapsing into inline tables.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        let body = toml_edit::ser::to_string_pretty(self)
            .map_err(|e| ConfigError::Parse(e.to_string()))?;
        let mut out = String::from(
            "# edscene 配置\n\
             # 删掉这个文件并重启即可回到出厂状态（素材文件不会被删除）。\n\n",
        );
        out.push_str(&body);
        Ok(out)
    }

    pub fn from_toml(text: &str) -> Result<Config, ConfigError> {
        let mut cfg: Config = toml_edit::de::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        cfg.normalize();
        Ok(cfg)
    }

    /// `Ok(None)` means "no file yet" and the caller should use the factory
    /// config; a corrupt file is reported so the UI can say so loudly instead
    /// of silently reverting someone's work.
    pub fn load(path: &Path) -> Result<Option<Config>, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.display().to_string(),
                    source,
                })
            }
        };
        Self::from_toml(&text).map(Some)
    }

    /// Write via a sibling temp file and a rename, keeping one backup. Two
    /// processes never write: the service owns the file and the UI mutates it
    /// through the pipe.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = self.to_toml()?;
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_file_name(format!(
            "{}.tmp-{}",
            path.file_name().and_then(|s| s.to_str()).unwrap_or("config.toml"),
            std::process::id()
        ));
        let mut f = std::fs::File::create(&tmp).map_err(|source| ConfigError::Write {
            path: tmp.display().to_string(),
            source,
        })?;
        f.write_all(text.as_bytes())
            .and_then(|_| f.sync_all())
            .map_err(|source| ConfigError::Write {
                path: tmp.display().to_string(),
                source,
            })?;
        drop(f);
        if path.exists() {
            let bak = path.with_file_name("config.toml.bak");
            let _ = std::fs::remove_file(&bak);
            let _ = std::fs::rename(path, &bak);
        }
        std::fs::rename(&tmp, path).map_err(|source| ConfigError::Write {
            path: path.display().to_string(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_round_trips_through_toml() {
        let a = Config::factory();
        let text = a.to_toml().unwrap();
        let b = Config::from_toml(&text).unwrap();
        assert_eq!(a, b, "round trip changed the config:\n{text}");
    }

    #[test]
    fn partial_and_empty_files_get_the_factory_defaults() {
        let empty = Config::from_toml("").unwrap();
        assert_eq!(empty, Config::factory());
        let partial = Config::from_toml("[audio]\nvolume = 50\n").unwrap();
        assert_eq!(partial.audio.volume, 50);
        assert_eq!(partial.events.len(), 8);
        assert!(partial.is_enabled(EventKind::Unlock));
        assert!(!partial.is_enabled(EventKind::Lock));
    }

    #[test]
    fn unknown_keys_are_tolerated_for_forward_compatibility() {
        let c = Config::from_toml("future_flag = true\n[events.unlock]\nenabled = false\n").unwrap();
        assert!(!c.is_enabled(EventKind::Unlock));
        assert!(c.is_enabled(EventKind::AcLost));
    }

    #[test]
    fn volumes_compose_and_stay_readable_in_the_file() {
        let mut c = Config::factory();
        c.audio.volume = 50;
        c.event_mut(EventKind::Unlock).volume = 40;
        assert!((c.volume_for(EventKind::Unlock) - 0.2).abs() < 1e-6);
        assert!((c.volume_for(EventKind::Lock) - 0.5).abs() < 1e-6);
        let text = c.to_toml().unwrap();
        assert!(text.contains("[audio]"), "{text}");
        assert!(text.contains("[events.unlock]"), "{text}");
        assert!(!text.contains("0.20000000"), "no f32 drift in the file");
    }

    #[test]
    fn dispatch_config_takes_the_wider_of_global_and_per_event_cooldowns() {
        let mut c = Config::factory();
        c.audio.min_interval_ms = 3_000;
        c.event_mut(EventKind::Unlock).min_interval_ms = 800;
        c.event_mut(EventKind::BatteryLow).min_interval_ms = 45_000;
        let d = c.dispatch_config();
        assert_eq!(d.cooldown_ms.get(EventKind::Unlock.id()), Some(&3_000));
        assert_eq!(d.cooldown_ms.get(EventKind::BatteryLow.id()), Some(&45_000));
        assert_eq!(d.cooldown_ms.len(), 8);
    }

    #[test]
    fn enabled_events_follow_the_display_order_not_the_config_order() {
        let c = Config::factory();
        let on = c.enabled_events();
        assert_eq!(on[0], EventKind::Unlock);
        assert_eq!(on.len(), 5);
        assert!(!on.contains(&EventKind::BootDesktop));
    }

    #[test]
    fn save_is_atomic_and_leaves_a_backup() {
        let dir = std::env::temp_dir().join(format!("edscene-cfg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        let mut c = Config::factory();
        c.sounds.dir = "D:\\works\\eds".into();
        c.save(&p).unwrap();
        let first = std::fs::read_to_string(&p).unwrap();
        c.audio.volume = 25;
        c.save(&p).unwrap();
        let second = std::fs::read_to_string(&p).unwrap();
        assert_ne!(first, second);
        assert!(dir.join("config.toml.bak").exists());
        assert_eq!(Config::load(&p).unwrap().unwrap().audio.volume, 25);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loading_a_missing_file_is_not_an_error() {
        let p = std::env::temp_dir().join("edscene-does-not-exist-42/config.toml");
        assert!(Config::load(&p).unwrap().is_none());
    }

    #[test]
    fn corrupt_files_are_reported_instead_of_shrugged_off() {
        assert!(matches!(
            Config::from_toml("[events.unlock\nenabled="),
            Err(ConfigError::Parse(_))
        ));
    }
}
