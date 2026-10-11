//! The one pipeline every mode shares: signals in, dispatcher, resolved audio
//! out. Only the delivery differs between modes, which is what [`Broadcast`]
//! abstracts.
use crate::log;
use crate::player::Engine;
use eds_core::config::Config;
use eds_core::dispatcher::{Dispatcher, Signal};
use eds_core::event::EventKind;
use eds_core::resolve::{self, Resolution, SoundStatus};
use eds_core::Paths;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Delivers a decided event. The agent plays it; the supervisor forwards it to
/// the session that owns the audio device.
pub trait Broadcast: Send + Sync {
    fn fire(&self, kind: EventKind, seq: u64, at_ms: u64);
    /// Block until everything queued has finished. Only the local player has
    /// anything to wait for.
    fn wait_idle(&self, _timeout: std::time::Duration) {}
}

pub struct LocalBroadcast {
    engine: Engine,
    cfg: Mutex<Config>,
    paths: Paths,
}

impl LocalBroadcast {
    pub fn new(cfg: Config, paths: Paths) -> LocalBroadcast {
        let engine = Engine::start(cfg.audio.clone());
        LocalBroadcast {
            engine,
            cfg: Mutex::new(cfg),
            paths,
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn set_config(&self, cfg: Config) {
        self.engine.set_audio(&cfg.audio);
        *self.cfg.lock().unwrap() = cfg;
    }

    /// Re-read the config file, which the UI owns.
    pub fn reload_config(&self) {
        if let Ok((cfg, _)) = crate::load_config(&self.paths) {
            self.set_config(cfg);
        }
    }

    pub fn resolve(&self, kind: EventKind) -> Resolution {
        let cfg = self.cfg.lock().unwrap();
        resolve::resolve(&cfg, &self.paths, kind)
    }
}

impl Broadcast for LocalBroadcast {
    fn fire(&self, kind: EventKind, seq: u64, at_ms: u64) {
        let (cfg, ecfg) = {
            let c = self.cfg.lock().unwrap();
            (c.clone(), c.event(kind))
        };
        if !ecfg.enabled {
            log::write(
                &self.paths.state_dir,
                &serde_json::json!({"t": at_ms, "ev": "skipped", "why": "disabled", "kind": kind.id(), "seq": seq}),
            );
            return;
        }
        let r = resolve::resolve(&cfg, &self.paths, kind);
        let volume = cfg.volume_for(kind);
        match (r.status, r.path.clone()) {
            (SoundStatus::Ready, Some(path)) => {
                match self.engine.play(&path, volume) {
                    Ok(true) => log::write(
                        &self.paths.state_dir,
                        &serde_json::json!({
                            "t": at_ms, "ev": "play", "kind": kind.id(), "seq": seq,
                            "file": path.display().to_string(), "volume": crate::log::gain(volume)
                        }),
                    ),
                    Ok(false) => log::write(
                        &self.paths.state_dir,
                        &serde_json::json!({"t": at_ms, "ev": "skipped", "why": "busy", "kind": kind.id(), "seq": seq}),
                    ),
                    Err(e) => log::write(
                        &self.paths.state_dir,
                        &serde_json::json!({"t": at_ms, "ev": "error", "kind": kind.id(), "err": e.to_string()}),
                    ),
                }
            }
            (status, path) => log::write(
                &self.paths.state_dir,
                &serde_json::json!({
                    "t": at_ms, "ev": "missing", "kind": kind.id(), "seq": seq,
                    "why": status.label(), "wanted": r.preset_file,
                    "path": path.map(|p| p.display().to_string()).unwrap_or_default(),
                    "dir": cfg.sounds.dir
                }),
            ),
        }
    }

    fn wait_idle(&self, timeout: std::time::Duration) {
        self.engine.wait_idle(timeout);
    }
}

/// Prints decisions instead of playing them: `--watch`, `--signals --quiet`
/// and CI all use it so a test run never grabs the audio device.
#[derive(Default)]
pub struct NullBroadcast;

impl Broadcast for NullBroadcast {
    fn fire(&self, kind: EventKind, seq: u64, at_ms: u64) {
        use std::io::Write as _;
        let mut so = std::io::stdout().lock();
        let _ = writeln!(so, "  (silent) #{seq} @{at_ms}ms {}", kind.id());
    }
}

/// Runs the state machine in front of a [`Broadcast`].
pub struct Pipeline {
    inner: Mutex<Dispatcher>,
    out: std::sync::Arc<dyn Broadcast>,
    seq: AtomicU64,
}

impl Pipeline {
    pub fn new(cfg: &Config, out: std::sync::Arc<dyn Broadcast>) -> Pipeline {
        let mut d = Dispatcher::new(cfg.dispatch_config(), log::mono_ms());
        d.set_machine_uptime(crate::platform::uptime_ms());
        Pipeline {
            inner: Mutex::new(d),
            out,
            seq: AtomicU64::new(0),
        }
    }

    /// Skip the state machine: used by `--simulate unlock` and the UI's 试听.
    pub fn force(&self, kind: EventKind, now_ms: u64) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.out.fire(kind, seq, now_ms);
    }

    /// One observation in, zero or more broadcasts out.
    pub fn feed(&self, sig: Signal, now_ms: u64) -> Vec<EventKind> {
        let fired = { self.inner.lock().unwrap().apply(sig, now_ms) };
        for k in &fired {
            let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
            self.out.fire(*k, seq, now_ms);
        }
        fired
    }

    /// The process started after the user was already at the desktop.
    pub fn assume_desktop(&self) {
        self.inner.lock().unwrap().assume_desktop();
    }
}
