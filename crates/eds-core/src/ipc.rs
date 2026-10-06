use crate::event::EventKind;
use serde::{Deserialize, Serialize};

/// Protocol revision; both ends refuse to talk across a mismatch rather than
/// guessing at field layouts.
pub const PROTOCOL: u32 = 1;
pub const PIPE_NAME: &str = "edscene.agent.v1";

pub fn pipe_path() -> String {
    if cfg!(windows) {
        format!(r"\\.\pipe\{PIPE_NAME}")
    } else {
        // interprocess uses a filesystem socket under the runtime dir elsewhere.
        format!("unix:path=$XDG_RUNTIME_DIR/{PIPE_NAME}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The always-on originator of events (Windows service / Linux daemon).
    Supervisor,
    /// The per-session process that owns the audio device and the window.
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Msg {
    Hello {
        role: Role,
        protocol: u32,
        pid: u32,
        session: u32,
    },
    /// "Broadcast this." `seq` is monotonic per supervisor so a reconnecting
    /// agent can drop anything it already played.
    Event {
        kind: EventKind,
        seq: u64,
        session: u32,
        /// Milliseconds since an arbitrary but fixed origin (monotonic clock).
        at_ms: u64,
    },
    /// The config file changed on disk; re-read and re-resolve.
    ConfigChanged {
        rev: u64,
    },
    /// The supervisor wants the agent to apply a change it cannot make itself
    /// (the supervisor owns the file, the UI owns the intent).
    Mutate {
        rev: u64,
        json: String,
    },
    /// Newest config revision the agent has loaded.
    Loaded {
        rev: u64,
    },
    Shutdown {
        reason: String,
    },
    /// Structured line for `--watch` / the log.
    Report {
        line: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("frame too large ({0} bytes), refusing")]
    TooLarge(u64),
    #[error("peer closed")]
    Closed,
}

/// Frames are `u32 LE length` followed by that many bytes of UTF-8 JSON.
pub const MAX_FRAME: u32 = 1 << 20;

pub fn encode(msg: &Msg) -> Result<Vec<u8>, IpcError> {
    let body = serde_json::to_vec(msg)?;
    if body.len() > MAX_FRAME as usize {
        return Err(IpcError::TooLarge(body.len() as u64));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn write_msg<W: std::io::Write>(w: &mut W, msg: &Msg) -> Result<(), IpcError> {
    let bytes = encode(msg)?;
    w.write_all(&bytes)?;
    w.flush()?;
    Ok(())
}

pub fn read_msg<R: std::io::Read>(r: &mut R) -> Result<Msg, IpcError> {
    let mut head = [0u8; 4];
    match r.read_exact(&mut head) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(IpcError::Closed),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(head);
    if len > MAX_FRAME {
        return Err(IpcError::TooLarge(len as u64));
    }
    let mut body = vec![0u8; len as usize];
    match r.read_exact(&mut body) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(IpcError::Closed),
        Err(e) => return Err(e.into()),
    }
    Ok(serde_json::from_slice(&body)?)
}

/// Drops events a session has already been told to play. The supervisor is the
/// only originator, so this is a reconnect safety net, not the main dedupe.
#[derive(Debug, Default, Clone)]
pub struct SeqGuard {
    last: u64,
    seen: Vec<u64>,
}

impl SeqGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn accept(&mut self, seq: u64) -> bool {
        if self.seen.contains(&seq) {
            return false;
        }
        self.seen.push(seq);
        if self.seen.len() > 64 {
            self.seen.remove(0);
        }
        self.last = self.last.max(seq);
        true
    }

    pub fn last(&self) -> u64 {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_variant_round_trips_through_a_frame() {
        let msgs = vec![
            Msg::Hello {
                role: Role::Supervisor,
                protocol: PROTOCOL,
                pid: 4242,
                session: 1,
            },
            Msg::Event {
                kind: EventKind::BatteryLow,
                seq: 7,
                session: 1,
                at_ms: 12345,
            },
            Msg::ConfigChanged { rev: 3 },
            Msg::Mutate {
                rev: 4,
                json: "{\"a\":1}".into(),
            },
            Msg::Loaded { rev: 4 },
            Msg::Shutdown {
                reason: "logoff".into(),
            },
            Msg::Report {
                line: "解锁返回桌面 → ready".into(),
            },
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            write_msg(&mut buf, m).unwrap();
        }
        let mut cur = std::io::Cursor::new(buf);
        for m in &msgs {
            assert_eq!(&read_msg(&mut cur).unwrap(), m);
        }
        assert!(matches!(read_msg(&mut cur), Err(IpcError::Closed)));
    }

    #[test]
    fn a_truncated_frame_is_a_clean_close_not_a_panic() {
        let bytes = encode(&Msg::ConfigChanged { rev: 9 }).unwrap();
        let mut cur = std::io::Cursor::new(&bytes[..bytes.len() - 2]);
        assert!(matches!(read_msg(&mut cur), Err(IpcError::Closed)));
    }

    #[test]
    fn absurd_frame_lengths_are_refused_before_allocating() {
        let mut cur = std::io::Cursor::new(vec![0xff, 0xff, 0xff, 0x7f]);
        assert!(matches!(
            read_msg(&mut cur),
            Err(IpcError::TooLarge(_))
        ));
    }

    #[test]
    fn seq_guard_drops_replays_and_forgets_old_history() {
        let mut g = SeqGuard::new();
        assert!(g.accept(1));
        assert!(!g.accept(1));
        assert!(g.accept(2));
        assert_eq!(g.last(), 2);
        for s in 3..80 {
            assert!(g.accept(s));
        }
        assert!(!g.accept(40), "the window keeps the last 64");
        assert!(g.accept(999));
    }

    #[test]
    fn event_kind_uses_its_stable_id_on_the_wire() {
        let b = serde_json::to_vec(&Msg::Event {
            kind: EventKind::AcLost,
            seq: 1,
            session: 0,
            at_ms: 0,
        })
        .unwrap();
        let s = String::from_utf8(b).unwrap();
        assert!(s.contains("\"ac_lost\""), "{s}");
    }
}
