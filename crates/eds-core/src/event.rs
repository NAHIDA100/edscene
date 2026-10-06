use serde::{Deserialize, Serialize};

/// Every system event edscene can broadcast. The variant order is the wire
/// order used by the config file, the IPC frames and the cooldown table, so
/// appending new kinds at the end is the only safe way to grow the enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    BootDesktop,
    Unlock,
    Lock,
    ScreenSaverRestore,
    BatterySaver,
    BatteryLow,
    AcLost,
    AcRestored,
}

impl EventKind {
    pub const ALL: [EventKind; 8] = [
        EventKind::BootDesktop,
        EventKind::Unlock,
        EventKind::Lock,
        EventKind::ScreenSaverRestore,
        EventKind::BatterySaver,
        EventKind::BatteryLow,
        EventKind::AcLost,
        EventKind::AcRestored,
    ];

    /// Stable ASCII identifier used as the config key and the override filename.
    pub const fn id(self) -> &'static str {
        match self {
            EventKind::BootDesktop => "boot_desktop",
            EventKind::Unlock => "unlock",
            EventKind::Lock => "lock",
            EventKind::ScreenSaverRestore => "screensaver_restore",
            EventKind::BatterySaver => "battery_saver",
            EventKind::BatteryLow => "battery_low",
            EventKind::AcLost => "ac_lost",
            EventKind::AcRestored => "ac_restored",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            EventKind::BootDesktop => 0,
            EventKind::Unlock => 1,
            EventKind::Lock => 2,
            EventKind::ScreenSaverRestore => 3,
            EventKind::BatterySaver => 4,
            EventKind::BatteryLow => 5,
            EventKind::AcLost => 6,
            EventKind::AcRestored => 7,
        }
    }

    pub const fn from_index(i: usize) -> Option<EventKind> {
        match i {
            0 => Some(EventKind::BootDesktop),
            1 => Some(EventKind::Unlock),
            2 => Some(EventKind::Lock),
            3 => Some(EventKind::ScreenSaverRestore),
            4 => Some(EventKind::BatterySaver),
            5 => Some(EventKind::BatteryLow),
            6 => Some(EventKind::AcLost),
            7 => Some(EventKind::AcRestored),
            _ => None,
        }
    }

    pub fn from_id(s: &str) -> Option<EventKind> {
        EventKind::ALL.into_iter().find(|k| k.id() == s)
    }
}

/// Session-scoped events are driven by logon/lock signals; power events are
/// derived from a polled power status. The UI groups rows the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Session,
    Power,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_round_trip() {
        let mut seen = std::collections::HashSet::new();
        for k in EventKind::ALL {
            assert!(seen.insert(k.id()), "duplicate id {}", k.id());
            assert_eq!(EventKind::from_id(k.id()), Some(k));
            assert_eq!(EventKind::from_index(k.index()), Some(k));
        }
        assert_eq!(EventKind::ALL.len(), 8);
    }
}
