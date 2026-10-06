use crate::event::{EventKind, Group};

/// A factory preset: what an event is called, which file name it looks for in
/// the user's sound pack, and whether it is on out of the box.
///
/// `preset_file` is only a *name*. edscene ships no audio whatsoever, so the
/// string is resolved against the sound directory the user points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventDef {
    pub kind: EventKind,
    pub title_zh: &'static str,
    pub title_en: &'static str,
    pub group: Group,
    pub preset_file: &'static str,
    pub default_enabled: bool,
    /// What has to happen on the host OS for this to fire, shown as row hint.
    pub hint_zh: &'static str,
    pub hint_en: &'static str,
}

impl EventDef {
    /// File name without extension, used for extension-agnostic matching so a
    /// user can replace `锁屏.mp3` with `锁屏.wav` and keep the preset working.
    pub fn stem(&self) -> &str {
        match self.preset_file.rfind('.') {
            Some(i) if i > 0 => &self.preset_file[..i],
            _ => self.preset_file,
        }
    }
}

/// The factory table. Order is the display order in the UI.
///
/// Defaults follow the shipped sound pack: session/power transitions the user
/// actually hears are on, the boot and lock chimes are off until asked for.
pub const EVENTS: &[EventDef; 8] = &[
    EventDef {
        kind: EventKind::Unlock,
        title_zh: "解锁返回桌面",
        title_en: "Desktop unlocked",
        group: Group::Session,
        preset_file: "解锁返回桌面.mp3",
        default_enabled: true,
        hint_zh: "在锁屏界面输入密码、回到桌面的瞬间",
        hint_en: "The moment the lock screen is dismissed and the desktop returns",
    },
    EventDef {
        kind: EventKind::BootDesktop,
        title_zh: "开机进入桌面",
        title_en: "Boot to desktop",
        group: Group::Session,
        preset_file: "开机进入桌面.mp3",
        default_enabled: false,
        hint_zh: "开机后第一次进入桌面（与解锁返回桌面二选一，避免重复）",
        hint_en: "First desktop entry after power-on (pick this or the unlock chime)",
    },
    EventDef {
        kind: EventKind::Lock,
        title_zh: "锁屏",
        title_en: "Screen locked",
        group: Group::Session,
        preset_file: "锁屏.mp3",
        default_enabled: false,
        hint_zh: "按下 Win+L 或账户被锁定的瞬间",
        hint_en: "Win+L or any other session lock",
    },
    EventDef {
        kind: EventKind::ScreenSaverRestore,
        title_zh: "屏保恢复",
        title_en: "Screen saver exited",
        group: Group::Session,
        preset_file: "屏保恢复.mp3",
        default_enabled: false,
        hint_zh: "屏幕保护结束、显示器重新点亮，且期间没有锁屏",
        hint_en: "Screensaver ended and display came back without locking",
    },
    EventDef {
        kind: EventKind::AcLost,
        title_zh: "断开电源（电源适配器）",
        title_en: "Adapter unplugged",
        group: Group::Power,
        preset_file: "断开电源（电源适配器）.mp3",
        default_enabled: true,
        hint_zh: "外接电源被拔掉的瞬间",
        hint_en: "The moment the AC adapter is pulled",
    },
    EventDef {
        kind: EventKind::AcRestored,
        title_zh: "电源恢复接入",
        title_en: "Adapter plugged in",
        group: Group::Power,
        preset_file: "电源恢复接入.mp3",
        default_enabled: true,
        hint_zh: "外接电源重新接上的瞬间",
        hint_en: "The moment the AC adapter is connected",
    },
    EventDef {
        kind: EventKind::BatterySaver,
        title_zh: "低电量模式",
        title_en: "Battery saver",
        group: Group::Power,
        preset_file: "低电量模式.mp3",
        default_enabled: true,
        hint_zh: "系统进入低电量（省电）模式的瞬间",
        hint_en: "The moment the OS enters battery-saver mode",
    },
    EventDef {
        kind: EventKind::BatteryLow,
        title_zh: "电池电量低于5%",
        title_en: "Battery below 5%",
        group: Group::Power,
        preset_file: "电池电量低于5%.mp3",
        default_enabled: true,
        hint_zh: "电量跌破阈值；需回升到复位线才会再次触发",
        hint_en: "Charge drops under the threshold; re-arms above the reset line",
    },
];

pub fn def(kind: EventKind) -> &'static EventDef {
    let mut i = 0;
    loop {
        let d = &EVENTS[i];
        if d.kind == kind {
            return d;
        }
        i += 1;
        if i >= EVENTS.len() {
            panic!("EventKind has no preset entry");
        }
    }
}

pub fn title(kind: EventKind, zh: bool) -> &'static str {
    let d = def(kind);
    if zh {
        d.title_zh
    } else {
        d.title_en
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_covers_every_kind_exactly_once() {
        for k in EventKind::ALL {
            assert_eq!(EVENTS.iter().filter(|d| d.kind == k).count(), 1, "{k:?}");
        }
        assert_eq!(EVENTS.len(), EventKind::ALL.len());
    }

    #[test]
    fn factory_defaults_match_the_shipped_sound_pack() {
        let on: Vec<&str> = EVENTS.iter().filter(|d| d.default_enabled).map(|d| d.kind.id()).collect();
        assert_eq!(
            on,
            vec!["unlock", "ac_lost", "ac_restored", "battery_saver", "battery_low"]
        );
        let off: Vec<&str> = EVENTS.iter().filter(|d| !d.default_enabled).map(|d| d.kind.id()).collect();
        assert_eq!(off, vec!["boot_desktop", "lock", "screensaver_restore"]);
        assert_eq!(on.len() + off.len(), 8);
    }

    #[test]
    fn preset_files_are_unique_and_have_extensions() {
        let mut seen = std::collections::HashSet::new();
        for d in EVENTS {
            assert!(d.preset_file.ends_with(".mp3"));
            assert!(seen.insert(d.preset_file), "duplicate preset {}", d.preset_file);
            assert!(!d.stem().contains('.'));
        }
    }

    #[test]
    fn stems_keep_the_full_width_and_percent_characters() {
        assert_eq!(def(EventKind::AcLost).stem(), "断开电源（电源适配器）");
        assert_eq!(def(EventKind::BatteryLow).stem(), "电池电量低于5%");
        assert_eq!(def(EventKind::Lock).stem(), "锁屏");
    }
}
