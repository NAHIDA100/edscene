use crate::event::EventKind;
use std::collections::BTreeMap;

/// A raw observation from a platform provider. Providers must not interpret:
/// they report what the OS said, and the [`Dispatcher`] decides which broadcast
/// (if any) it means. Timestamps are supplied by the caller so the whole state
/// machine can be driven by a fake clock in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// A user session was created (credentials accepted, desktop being built).
    Logon { session: u32 },
    /// The console session was unlocked (lock screen dismissed).
    Unlock { session: u32 },
    /// The console session was locked.
    Lock { session: u32 },
    /// A session ended (fast user switch, logoff).
    Logoff { session: u32 },
    /// The physical console was attached to a session.
    ConsoleConnect { session: u32 },
    /// The physical console was detached.
    ConsoleDisconnect,
    /// Display switched on (monitor wake, screensaver exit).
    DisplayOn,
    /// Display switched off (monitor sleep, screensaver start).
    DisplayOff,
    /// Periodic power status sample. `saver` is `None` where the OS does not
    /// expose a battery-saver flag and it has to be derived.
    Power {
        on_ac: bool,
        percent: u8,
        saver: Option<bool>,
    },
    /// About to suspend / hibernate.
    Suspending,
    /// Resumed from suspend / hibernate.
    Resuming,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchConfig {
    /// A logon this soon after start-up counts as "boot into desktop".
    pub boot_grace_ms: u64,
    /// Battery level that arms the low-battery chime again.
    pub low_battery_rearm_percent: u8,
    /// Per-event minimum gap between two broadcasts.
    pub cooldown_ms: BTreeMap<String, u64>,
    /// Fallback battery-saver threshold when the OS gives no flag.
    pub saver_threshold_percent: u8,
}

impl Default for DispatchConfig {
    fn default() -> Self {
        let mut cd = BTreeMap::new();
        for k in EventKind::ALL {
            cd.insert(k.id().to_string(), 1500);
        }
        // Power polling can jitter; the low-battery chime is the one that must
        // never double-fire.
        cd.insert(EventKind::BatteryLow.id().to_string(), 60_000);
        DispatchConfig {
            boot_grace_ms: 90_000,
            low_battery_rearm_percent: 8,
            cooldown_ms: cd,
            saver_threshold_percent: 20,
        }
    }
}

/// The last thing the dispatcher knows about the host. Surfaced in the UI and
/// in `--watch` output so a mis-detection can be told apart from a mis-config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub locked: Option<bool>,
    pub on_ac: Option<bool>,
    pub percent: Option<u8>,
    pub saver: Option<bool>,
    pub display_on: Option<bool>,
    pub session: Option<u32>,
    pub low_armed: bool,
    pub first_logon_seen: bool,
}

/// Pure, deterministic rule set. One instance per process; the Windows service
/// and the Linux daemon each own the only one that matters.
#[derive(Debug, Clone)]
pub struct Dispatcher {
    cfg: DispatchConfig,
    t0_ms: u64,
    st: Snapshot,
    last_fire_ms: [Option<u64>; 8],
    prev_on_ac: Option<bool>,
    prev_saver: Option<bool>,
    /// Set while suspended so the resume does not read as a screensaver exit.
    suspended: bool,
    /// Power observations before the first one only seed state, they never fire.
    power_seeded: bool,
    /// Milliseconds of "still booting" left, measured on this clock. The service
    /// shortens it by the machine uptime, so a service installed three hours
    /// after boot no longer treats the next logon as a boot.
    boot_phase_ms: u64,
    /// Until when a Lock/Unlock pair is just the logon animation rather than the
    /// user locking their screen. See [`BOOT_DANCE_MS`].
    boot_dance_until_ms: u64,
}

/// Windows plays Logon, then Lock, then Unlock while building the desktop, all
/// inside a couple of seconds. That dance must not become a 锁屏 + 解锁 pair of
/// broadcasts, so it is swallowed for this long after the boot logon.
pub const BOOT_DANCE_MS: u64 = 10_000;

impl Dispatcher {
    pub fn new(cfg: DispatchConfig, now_ms: u64) -> Self {
        let boot_phase_ms = cfg.boot_grace_ms;
        Dispatcher {
            cfg,
            t0_ms: now_ms,
            st: Snapshot {
                low_armed: true,
                ..Default::default()
            },
            last_fire_ms: [None; 8],
            prev_on_ac: None,
            prev_saver: None,
            suspended: false,
            power_seeded: false,
            boot_phase_ms,
            boot_dance_until_ms: 0,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.st
    }

    /// Take new rules without forgetting anything else. Rebuilding the
    /// dispatcher instead would reset the boot clock, and a reload happening
    /// during the logon animation would then re-broadcast the boot.
    pub fn reload_config(&mut self, cfg: DispatchConfig) {
        self.boot_phase_ms = self.boot_phase_ms.min(cfg.boot_grace_ms);
        self.cfg = cfg;
    }

    /// Tell the dispatcher how long the machine had already been up when this
    /// process started. Without it, a service started by hand hours after boot
    /// would read the next logon as "开机进入桌面".
    pub fn set_machine_uptime(&mut self, uptime_ms: u64) {
        self.boot_phase_ms = self.cfg.boot_grace_ms.saturating_sub(uptime_ms);
    }

    /// Still inside the window where a logon means "the machine just booted".
    fn in_boot_phase(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.t0_ms) <= self.boot_phase_ms
    }

    /// The logon animation swallows Lock/Unlock only briefly, so a genuine
    /// lock a minute later is still a genuine broadcast.
    fn in_boot_dance(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.t0_ms) <= self.boot_dance_until_ms
    }

    /// Called by a process that starts *after* the user is already at the
    /// desktop (the agent, the UI): the session is unlocked and the first logon
    /// has come and gone, so nothing should be broadcast until a real edge.
    pub fn assume_desktop(&mut self) {
        self.st.locked = Some(false);
        self.st.display_on = Some(true);
        self.st.first_logon_seen = true;
    }

    pub fn config(&self) -> &DispatchConfig {
        &self.cfg
    }

    fn cooldown(&self, kind: EventKind) -> u64 {
        self.cfg
            .cooldown_ms
            .get(kind.id())
            .copied()
            .unwrap_or(1500)
    }

    /// Feed one observation, get back the broadcasts it implies (usually zero
    /// or one; a single power sample can imply both AC-lost and battery-saver).
    pub fn apply(&mut self, sig: Signal, now_ms: u64) -> Vec<EventKind> {
        let mut out = Vec::new();
        match sig {
            Signal::Logon { session } => {
                self.st.session = Some(session);
                self.st.locked = Some(false);
                self.st.display_on = Some(true);
                if !self.st.first_logon_seen {
                    self.st.first_logon_seen = true;
                    if self.in_boot_phase(now_ms) {
                        // One broadcast for the whole logon: the Lock/Unlock the
                        // OS sends next belongs to this same moment.
                        self.boot_dance_until_ms = now_ms.saturating_add(BOOT_DANCE_MS);
                        out.push(EventKind::BootDesktop);
                    } else {
                        out.push(EventKind::Unlock);
                    }
                } else {
                    // A later logon in the same boot is a fresh desktop entry.
                    out.push(EventKind::Unlock);
                }
            }
            Signal::Unlock { session } => {
                self.st.session = Some(session);
                self.st.display_on = Some(true);
                if self.in_boot_dance(now_ms) {
                    self.boot_dance_until_ms = 0;
                    self.st.locked = Some(false);
                } else if self.st.locked != Some(false) {
                    self.st.locked = Some(false);
                    out.push(EventKind::Unlock);
                }
            }
            Signal::Lock { .. } => {
                if self.st.locked != Some(true) {
                    self.st.locked = Some(true);
                    if !self.in_boot_dance(now_ms) {
                        out.push(EventKind::Lock);
                    }
                }
            }
            Signal::Logoff { session } => {
                self.st.locked = None;
                self.st.display_on = None;
                if self.st.session == Some(session) {
                    self.st.session = None;
                }
            }
            Signal::ConsoleConnect { session } => {
                self.st.session = Some(session);
                self.st.display_on = Some(true);
                // After a lock the OS always sends Unlock too; only use this as
                // a fallback when we never saw the lock state clear.
                if self.in_boot_dance(now_ms) {
                    self.boot_dance_until_ms = 0;
                    self.st.locked = Some(false);
                } else if self.st.locked == Some(true) {
                    self.st.locked = Some(false);
                    out.push(EventKind::Unlock);
                }
            }
            Signal::ConsoleDisconnect => {
                self.st.display_on = None;
            }
            Signal::DisplayOn => {
                let was_off = self.st.display_on == Some(false);
                self.st.display_on = Some(true);
                let quiet = !self.suspended && self.st.locked == Some(false);
                if was_off && quiet {
                    out.push(EventKind::ScreenSaverRestore);
                }
            }
            Signal::DisplayOff => {
                self.st.display_on = Some(false);
            }
            Signal::Suspending => {
                self.suspended = true;
                self.st.display_on = Some(false);
            }
            Signal::Resuming => {
                self.suspended = false;
                self.st.display_on = None;
                // The real Unlock / DisplayOn arrives right after and decides.
            }
            Signal::Power {
                on_ac,
                percent,
                saver,
            } => {
                let percent = percent.min(100);
                // Not every OS exposes a battery-saver flag; offline plus a low
                // charge is the documented approximation.
                let derived = !on_ac && percent <= self.cfg.saver_threshold_percent;
                let saver_now = Some(saver.unwrap_or(derived));
                let prev_ac = self.prev_on_ac;
                let prev_saver = self.prev_saver;
                self.prev_on_ac = Some(on_ac);
                self.prev_saver = saver_now;
                self.st.on_ac = Some(on_ac);
                self.st.percent = Some(percent);
                self.st.saver = saver_now;

                if !self.power_seeded {
                    self.power_seeded = true;
                    self.st.low_armed = percent > self.cfg.low_battery_rearm_percent;
                    return Vec::new();
                }
                if let Some(prev) = prev_ac {
                    if prev && !on_ac {
                        out.push(EventKind::AcLost);
                    } else if !prev && on_ac {
                        out.push(EventKind::AcRestored);
                    }
                }
                if prev_saver == Some(false) && saver_now == Some(true) {
                    out.push(EventKind::BatterySaver);
                }
                if self.st.low_armed && percent <= 5 {
                    self.st.low_armed = false;
                    out.push(EventKind::BatteryLow);
                } else if !self.st.low_armed && percent >= self.cfg.low_battery_rearm_percent {
                    self.st.low_armed = true;
                }
            }
        }
        out.retain(|k| self.guard(*k, now_ms));
        out
    }

    /// Cooldown gate; also records the fire time.
    fn guard(&mut self, kind: EventKind, now_ms: u64) -> bool {
        let i = kind.index();
        if let Some(prev) = self.last_fire_ms[i] {
            if now_ms.saturating_sub(prev) < self.cooldown(kind) {
                return false;
            }
        }
        self.last_fire_ms[i] = Some(now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d() -> Dispatcher {
        Dispatcher::new(DispatchConfig::default(), 0)
    }

    #[test]
    fn first_logon_within_grace_is_a_boot_and_only_once() {
        let mut x = d();
        assert_eq!(x.apply(Signal::Logon { session: 1 }, 4_000), vec![EventKind::BootDesktop]);
        assert_eq!(x.apply(Signal::Logon { session: 1 }, 60_000), vec![EventKind::Unlock]);
    }

    #[test]
    fn late_logon_is_reported_as_unlock_not_boot() {
        let mut x = d();
        assert_eq!(
            x.apply(Signal::Logon { session: 1 }, 120_000),
            vec![EventKind::Unlock]
        );
    }

    #[test]
    fn the_boot_logon_animation_is_not_a_lock_unlock_pair() {
        let mut x = d();
        assert_eq!(
            x.apply(Signal::Logon { session: 1 }, 4_000),
            vec![EventKind::BootDesktop]
        );
        // What Windows really sends while building the desktop.
        assert!(x.apply(Signal::Lock { session: 1 }, 5_000).is_empty());
        assert!(x.apply(Signal::Unlock { session: 1 }, 6_000).is_empty());
        assert!(x
            .apply(Signal::ConsoleConnect { session: 1 }, 7_000)
            .is_empty());
        // Once the animation is over a real lock/unlock is a real broadcast.
        assert_eq!(x.apply(Signal::Lock { session: 1 }, 30_000), vec![EventKind::Lock]);
        assert_eq!(
            x.apply(Signal::Unlock { session: 1 }, 40_000),
            vec![EventKind::Unlock]
        );
    }

    #[test]
    fn uptime_shortens_the_boot_window() {
        let mut x = d();
        x.set_machine_uptime(3_600_000);
        assert_eq!(
            x.apply(Signal::Logon { session: 1 }, 1_000),
            vec![EventKind::Unlock]
        );
    }

    #[test]
    fn reloading_config_neither_replays_the_boot_nor_ends_the_animation() {
        let mut x = d();
        assert_eq!(
            x.apply(Signal::Logon { session: 1 }, 4_000),
            vec![EventKind::BootDesktop]
        );
        x.reload_config(DispatchConfig::default());
        assert!(x.apply(Signal::Lock { session: 1 }, 5_000).is_empty());
        assert!(x.apply(Signal::Unlock { session: 1 }, 6_000).is_empty());
    }

    #[test]
    fn unlock_only_fires_on_a_real_locked_to_unlocked_edge() {
        let mut x = d();
        x.apply(Signal::Logon { session: 1 }, 1_000);
        assert!(x.apply(Signal::Unlock { session: 1 }, 5_000).is_empty());
        assert_eq!(x.apply(Signal::Lock { session: 1 }, 9_000), vec![EventKind::Lock]);
        assert!(x.apply(Signal::Lock { session: 1 }, 20_000).is_empty());
        assert_eq!(x.apply(Signal::Unlock { session: 1 }, 30_000), vec![EventKind::Unlock]);
    }

    #[test]
    fn console_connect_unlocks_but_never_double_fires_with_unlock() {
        let mut x = d();
        x.assume_desktop();
        x.apply(Signal::Lock { session: 1 }, 5_000);
        assert_eq!(
            x.apply(Signal::ConsoleConnect { session: 1 }, 9_000),
            vec![EventKind::Unlock]
        );
        assert!(x.apply(Signal::Unlock { session: 1 }, 12_000).is_empty());
    }

    #[test]
    fn screensaver_restore_needs_display_off_then_on_while_unlocked() {
        let mut x = d();
        x.apply(Signal::Logon { session: 1 }, 1_000);
        assert!(x.apply(Signal::DisplayOn, 2_000).is_empty());
        x.apply(Signal::DisplayOff, 5_000);
        assert_eq!(
            x.apply(Signal::DisplayOn, 9_000),
            vec![EventKind::ScreenSaverRestore]
        );
        // Locked while the display was off: the unlock chime owns that one.
        x.apply(Signal::DisplayOff, 20_000);
        x.apply(Signal::Lock { session: 1 }, 21_000);
        assert!(x.apply(Signal::DisplayOn, 30_000).is_empty());
        assert_eq!(x.apply(Signal::Unlock { session: 1 }, 31_000), vec![EventKind::Unlock]);
    }

    #[test]
    fn resume_is_not_a_screensaver_restore() {
        let mut x = d();
        x.apply(Signal::Logon { session: 1 }, 1_000);
        x.apply(Signal::DisplayOff, 5_000);
        x.apply(Signal::Suspending, 6_000);
        x.apply(Signal::Resuming, 7_000);
        assert!(x.apply(Signal::DisplayOn, 9_000).is_empty());
    }

    #[test]
    fn first_power_sample_never_broadcasts() {
        let mut x = d();
        assert!(x.apply(Signal::Power { on_ac: false, percent: 3, saver: None }, 1_000).is_empty());
        assert!(!x.snapshot().low_armed, "3% must consume the first arming");
    }

    #[test]
    fn battery_low_uses_hysteresis_and_fires_exactly_twice() {
        let mut x = d();
        let trace = [12u8, 8, 6, 5, 4, 5, 6, 7, 20, 4, 3];
        let mut hits = Vec::new();
        for (i, p) in trace.iter().enumerate() {
            let out = x.apply(
                Signal::Power { on_ac: false, percent: *p, saver: None },
                10_000 + (i as u64) * 120_000,
            );
            for k in out {
                hits.push((*p, k));
            }
        }
        let lows: Vec<u8> = hits.iter().filter(|(_, k)| *k == EventKind::BatteryLow).map(|(p, _)| *p).collect();
        assert_eq!(lows, vec![5, 4], "must not re-fire until it re-arms at 8%: {lows:?}");
    }

    #[test]
    fn ac_edges_are_symmetric() {
        let mut x = d();
        x.apply(Signal::Power { on_ac: true, percent: 80, saver: None }, 1_000);
        assert_eq!(
            x.apply(Signal::Power { on_ac: false, percent: 79, saver: None }, 5_000),
            vec![EventKind::AcLost]
        );
        assert_eq!(
            x.apply(Signal::Power { on_ac: true, percent: 80, saver: None }, 9_000),
            vec![EventKind::AcRestored]
        );
        assert!(x.apply(Signal::Power { on_ac: true, percent: 81, saver: None }, 13_000).is_empty());
    }

    #[test]
    fn battery_saver_prefers_the_os_flag_and_falls_back_to_deriving() {
        let mut x = d();
        x.apply(Signal::Power { on_ac: true, percent: 90, saver: Some(false) }, 1_000);
        assert_eq!(
            x.apply(Signal::Power { on_ac: true, percent: 90, saver: Some(true) }, 5_000),
            vec![EventKind::BatterySaver]
        );
        let mut y = d();
        y.apply(Signal::Power { on_ac: true, percent: 90, saver: None }, 1_000);
        // Offline at 18% with a 20% threshold: derived saver, plus the AC edge.
        let out = y.apply(Signal::Power { on_ac: false, percent: 18, saver: None }, 5_000);
        assert_eq!(out, vec![EventKind::AcLost, EventKind::BatterySaver]);
    }

    #[test]
    fn cooldown_throttles_repeats_but_not_the_state_machine() {
        let mut x = d();
        x.assume_desktop();
        assert_eq!(x.apply(Signal::Lock { session: 1 }, 2_000), vec![EventKind::Lock]);
        assert_eq!(x.apply(Signal::Unlock { session: 1 }, 3_000), vec![EventKind::Unlock]);
        // Both edges are still tracked...
        x.apply(Signal::Lock { session: 1 }, 3_500);
        // ...but the broadcasts are inside the 1.5 s cooldown window.
        assert!(x.apply(Signal::Unlock { session: 1 }, 4_000).is_empty());
        assert_eq!(x.snapshot().locked, Some(false));
        x.apply(Signal::Lock { session: 1 }, 6_000);
        assert_eq!(x.apply(Signal::Unlock { session: 1 }, 6_100), vec![EventKind::Unlock]);
    }

    #[test]
    fn assuming_the_desktop_silences_the_starting_edge() {
        let mut x = d();
        x.assume_desktop();
        assert!(
            x.apply(Signal::Unlock { session: 1 }, 5_000).is_empty(),
            "an agent that started at the desktop must not greet itself"
        );
        x.apply(Signal::Lock { session: 1 }, 20_000);
        assert_eq!(x.apply(Signal::Unlock { session: 1 }, 30_000), vec![EventKind::Unlock]);
    }

    #[test]
    fn logoff_resets_the_lock_state() {
        let mut x = d();
        x.apply(Signal::Logon { session: 1 }, 1_000);
        x.apply(Signal::Lock { session: 1 }, 5_000);
        x.apply(Signal::Logoff { session: 1 }, 9_000);
        assert_eq!(x.snapshot().locked, None);
        assert_eq!(x.snapshot().session, None);
    }
}
