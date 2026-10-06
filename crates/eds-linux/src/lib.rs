//! Linux event sources over D-Bus.
//!
//! Deliberately **polling** rather than signal-matching: three property reads
//! every 400 ms on a local unix socket costs less CPU than one wake-up, and it
//! keeps this crate free of an async runtime. The edge detection, hysteresis
//! and boot-vs-unlock logic all live in `eds_core::Dispatcher`, so a poll and a
//! signal produce exactly the same broadcasts — only the latency differs (at
//! most one interval, well under a perceptible beat for a greeting chime).
//!
//! Buses:
//! * **system bus** — `org.freedesktop.login1` (sessions, lock hint,
//!   greeter-vs-user class) and `org.freedesktop.UPower` (AC, charge).
//! * **session bus** — `org.gnome.ScreenSaver` / `org.freedesktop.ScreenSaver`
//!   (`GetActive`), reachable only from a process inside the graphical
//!   session, i.e. the user-level unit.
//!
//! A system-level daemon can still read login1, which is enough for
//! boot/greeter transitions; the screensaver source simply stays quiet when
//! there is no session bus to talk to.

pub mod desktop;

use eds_core::Signal;
use std::sync::mpsc::Sender;
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// One tick of the poll loop.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Observed {
    /// A real (non-greeter) session is active on seat 0.
    pub user_session: bool,
    /// That session is currently locked.
    pub locked: Option<bool>,
    /// Screensaver / DPMS is active, as reported by the session bus.
    pub saver_active: Option<bool>,
    pub on_ac: Option<bool>,
    pub percent: Option<u8>,
    pub session_id: u32,
}

pub fn poll_once(sys: &Connection, user: Option<&Connection>) -> Observed {
    let mut o = Observed::default();
    if let Some(s) = active_session(sys) {
        o.user_session = s.class == "user";
        o.locked = s.locked_hint;
        o.session_id = s.serial;
    }
    if let Some(c) = user {
        o.saver_active = screensaver_active(c);
    }
    if let Some(p) = upower(sys) {
        o.on_ac = Some(!p.on_battery);
        o.percent = p.percent;
    }
    o
}

/// Turn an observation into raw signals for the dispatcher. State is kept here
/// only for the edges that the OS does not report as events.
#[derive(Debug, Default)]
pub struct Feed {
    last: Observed,
    seeded: bool,
}

impl Feed {
    pub fn push(&mut self, now: Observed, tx: &Sender<Signal>) {
        if !self.seeded {
            self.seeded = true;
            self.last = now.clone();
            // Seed the power reading so a machine that boots on battery does
            // not immediately announce "adapter unplugged".
            let _ = tx.send(Signal::Power {
                on_ac: now.on_ac.unwrap_or(true),
                percent: now.percent.unwrap_or(100),
                saver: None,
            });
            return;
        }
        if now.user_session && !self.last.user_session {
            let _ = tx.send(Signal::Logon {
                session: now.session_id,
            });
        }
        let was = self.last.locked;
        if let (Some(false), Some(true)) = (now.locked, was) {
            let _ = tx.send(Signal::Lock {
                session: now.session_id,
            });
        }
        if let (Some(true), Some(false)) = (now.locked, was) {
            let _ = tx.send(Signal::Unlock {
                session: now.session_id,
            });
        }
        match (now.saver_active, self.last.saver_active) {
            (Some(true), Some(false)) => {
                let _ = tx.send(Signal::DisplayOff);
            }
            (Some(false), Some(true)) => {
                let _ = tx.send(Signal::DisplayOn);
            }
            _ => {}
        }
        if now.on_ac.is_some() || now.percent.is_some() {
            let _ = tx.send(Signal::Power {
                on_ac: now.on_ac.unwrap_or(true),
                percent: now.percent.unwrap_or(100),
                saver: None,
            });
        }
        self.last = now;
    }
}

struct Session {
    class: String,
    locked_hint: Option<bool>,
    /// login1 has no small-int session id; the object path's serial is stable
    /// and unique enough to key the per-session maps.
    serial: u32,
}

fn active_session(conn: &Connection) -> Option<Session> {
    let reply = conn
        .call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            Some("org.freedesktop.login1.Manager"),
            "ListSessions",
            &(),
        )
        .ok()?;
    let sessions: Vec<(String, u32, OwnedObjectPath, String, String, bool)> =
        reply.body().deserialize().ok()?;
    let (_, serial, path, _type, class, active) = sessions.into_iter().rev().find(|s| s.5)?;
    let _ = active;
    let locked_hint = bool_prop(
        conn,
        "org.freedesktop.login1",
        path.as_str(),
        "org.freedesktop.login1.Session",
        "LockedHint",
    );
    Some(Session {
        class,
        locked_hint,
        serial,
    })
}

struct Power {
    on_battery: bool,
    percent: Option<u8>,
}

fn upower(conn: &Connection) -> Option<Power> {
    let on_battery = bool_prop(
        conn,
        "org.freedesktop.UPower",
        "/org/freedesktop/UPower",
        "org.freedesktop.UPower",
        "OnBattery",
    )?;
    let percent = f64_prop(
        conn,
        "org.freedesktop.UPower",
        "/org/freedesktop/UPower/devices/DisplayDevice",
        "org.freedesktop.UPower.Device",
        "Percentage",
    )
    .map(|p| p.clamp(0.0, 100.0) as u8);
    Some(Power { on_battery, percent })
}

fn screensaver_active(conn: &Connection) -> Option<bool> {
    for (service, path, iface, method) in [
        (
            "org.gnome.ScreenSaver",
            "/org/gnome/ScreenSaver",
            "org.gnome.ScreenSaver",
            "GetActive",
        ),
        (
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
            "org.freedesktop.ScreenSaver",
            "GetActive",
        ),
    ] {
        if let Ok(r) = conn.call_method(Some(service), path, Some(iface), method, &()) {
            if let Ok(v) = r.body().deserialize::<bool>() {
                return Some(v);
            }
        }
    }
    // KDE and others only expose the lock hint on the session bus.
    bool_prop(
        conn,
        "org.keyscreen",
        "/MainApplication",
        "org.kde.kscreen",
        "locked",
    )
}

fn get_prop(
    conn: &Connection,
    service: &str,
    path: &str,
    iface: &str,
    name: &str,
) -> Option<OwnedValue> {
    let reply = conn
        .call_method(
            Some(service),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(iface, name),
        )
        .ok()?;
    let body = reply.body();
    let value: OwnedValue = body.deserialize().ok()?;
    Some(value)
}

fn bool_prop(conn: &Connection, service: &str, path: &str, iface: &str, name: &str) -> Option<bool> {
    let v = get_prop(conn, service, path, iface, name)?;
    match v.try_into() {
        Ok(b) => Some(b),
        Err(_) => None,
    }
}

fn f64_prop(conn: &Connection, service: &str, path: &str, iface: &str, name: &str) -> Option<f64> {
    let v = get_prop(conn, service, path, iface, name)?;
    f64::try_from(v).ok()
}

/// Open the system bus, and the session bus when one is reachable.
pub struct Buses {
    pub system: Connection,
    pub user: Option<Connection>,
}

impl Buses {
    pub fn open() -> Result<Buses, String> {
        let system = Connection::system().map_err(|e| format!("无法连接系统 D-Bus: {e}"))?;
        let user = Connection::session().ok();
        Ok(Buses { system, user })
    }

    pub fn poll(&self) -> Observed {
        poll_once(&self.system, self.user.as_ref())
    }
}

/// Is this a systemd user session we can install into?
pub fn systemd_user_available() -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-system-running"])
        .output()
        .map(|o| o.status.success() || !o.stdout.is_empty())
        .unwrap_or(false)
}

/// `XDG_SESSION_TYPE` / `WAYLAND_DISPLAY` only decide which screenshot tool can
/// see the desktop; they never change the event logic.
pub fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").unwrap_or_default() == "wayland"
}

