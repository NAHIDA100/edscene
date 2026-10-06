//! Audio engine. One output stream exists only while it is needed: the device
//! is opened on the first broadcast and released a few seconds after the last
//! one, so an idle agent holds no audio endpoint and no mixer thread.
use eds_core::config::{Audio, Policy};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum PlayError {
    #[error("no audio output device: {0}")]
    NoDevice(String),
    #[error("cannot read {path}: {reason}")]
    Read { path: String, reason: String },
    #[error("cannot decode {path} (unsupported or corrupt): {reason}")]
    Decode { path: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Played {
    pub device: String,
    pub file: String,
}

struct Holder {
    _sink: rodio::MixerDeviceSink,
    player: rodio::Player,
}

struct State {
    holder: Option<Holder>,
    last_play: Instant,
    dropped_streams: u64,
}

fn describe(sink: &rodio::MixerDeviceSink, src: &rodio::Decoder<std::io::BufReader<std::fs::File>>) -> Played {
    use rodio::Source as _;
    Played {
        device: format!(
            "{:?}Hz/{}ch/{:?}",
            sink.config().sample_rate(),
            sink.config().channel_count(),
            sink.config().sample_format()
        ),
        file: format!("{:?}Hz/{}ch", src.sample_rate(), src.channels()),
    }
}

fn open_file(path: &Path) -> Result<rodio::Decoder<std::io::BufReader<std::fs::File>>, PlayError> {
    let file = std::fs::File::open(path).map_err(|e| PlayError::Read {
        path: path.display().to_string(),
        reason: e.to_string(),
    })?;
    rodio::Decoder::try_from(std::io::BufReader::new(file)).map_err(|e| PlayError::Decode {
        path: path.display().to_string(),
        reason: format!("{e:?}"),
    })
}

/// A one-shot blocking playback, used by `--selftest` and the UI's 试听 button.
/// Deliberately separate from [`Engine`]: it needs no background thread.
pub fn play_blocking(path: &Path, volume: f32, timeout: Duration) -> Result<Played, PlayError> {
    let mut sink = rodio::DeviceSinkBuilder::open_default_sink()
        .map_err(|e| PlayError::NoDevice(format!("{e:?}")))?;
    sink.log_on_drop(false);
    let player = rodio::Player::connect_new(sink.mixer());
    let decoder = open_file(path)?;
    let info = describe(&sink, &decoder);
    player.set_volume(volume.clamp(0.0, 1.0));
    player.append(decoder);
    let deadline = Instant::now() + timeout;
    while !player.empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(40));
    }
    player.stop();
    drop(player);
    drop(sink);
    Ok(info)
}

/// Long-lived engine used by the agent. `play` never blocks.
#[derive(Clone)]
pub struct Engine {
    st: Arc<Mutex<State>>,
    cv: Arc<Condvar>,
    cfg: Arc<Mutex<Audio>>,
}

impl Engine {
    pub fn start(audio: Audio) -> Engine {
        let e = Engine {
            st: Arc::new(Mutex::new(State {
                holder: None,
                last_play: Instant::now(),
                dropped_streams: 0,
            })),
            cv: Arc::new(Condvar::new()),
            cfg: Arc::new(Mutex::new(audio)),
        };
        let (st, cv, cfg) = (e.st.clone(), e.cv.clone(), e.cfg.clone());
        std::thread::Builder::new()
            .name("edscene-reaper".into())
            .stack_size(128 * 1024)
            .spawn(move || reap(st, cv, cfg))
            .ok();
        e
    }

    pub fn set_audio(&self, audio: &Audio) {
        *self.cfg.lock().unwrap() = audio.clone();
        self.cv.notify_all();
    }

    /// Returns `Ok(false)` when the policy decided to skip this broadcast.
    pub fn play(&self, path: &Path, volume: f32) -> Result<bool, PlayError> {
        let policy = self.cfg.lock().unwrap().policy;
        let mut st = self.st.lock().unwrap();
        if let Some(h) = st.holder.as_ref() {
            match policy {
                Policy::Interrupt => h.player.stop(),
                Policy::Queue => {}
                Policy::DropNewest => {
                    if !h.player.empty() {
                        return Ok(false);
                    }
                }
            }
        }
        if st.holder.is_none() {
            let mut sink = rodio::DeviceSinkBuilder::open_default_sink()
                .map_err(|e| PlayError::NoDevice(format!("{e:?}")))?;
            sink.log_on_drop(false);
            let player = rodio::Player::connect_new(sink.mixer());
            st.holder = Some(Holder {
                _sink: sink,
                player,
            });
        }
        let h = st.holder.as_mut().unwrap();
        let decoder = open_file(path)?;
        h.player.set_volume(volume.clamp(0.0, 1.0));
        h.player.append(decoder);
        st.last_play = Instant::now();
        self.cv.notify_all();
        Ok(true)
    }

    /// Block until nothing is queued any more (used by tests and `--simulate`).
    pub fn wait_idle(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            let empty = self
                .st
                .lock()
                .unwrap()
                .holder
                .as_ref()
                .map(|h| h.player.empty())
                .unwrap_or(true);
            if empty || Instant::now() > deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    pub fn stats(&self) -> (u64, bool) {
        let st = self.st.lock().unwrap();
        (st.dropped_streams, st.holder.is_some())
    }
}

/// Parked while idle; wakes only to release the device.
fn reap(st: Arc<Mutex<State>>, cv: Arc<Condvar>, cfg: Arc<Mutex<Audio>>) {
    loop {
        let idle_ms = cfg.lock().unwrap().output_idle_ms.max(200);
        let mut g = st.lock().unwrap();
        if g.holder.is_none() {
            g = cv.wait(g).unwrap();
            continue;
        }
        let empty = g.holder.as_ref().map(|h| h.player.empty()).unwrap_or(true);
        let since = g.last_play.elapsed();
        let need = Duration::from_millis(idle_ms);
        if empty && since >= need {
            g.holder = None;
            g.dropped_streams += 1;
            continue;
        }
        let wait = if empty {
            need.saturating_sub(since)
        } else {
            Duration::from_millis(250)
        };
        let (g2, _) = cv.wait_timeout(g, wait).unwrap();
        g = g2;
    }
}
