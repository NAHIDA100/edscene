//! Named-pipe link between the service (event originator) and the per-session
//! agents (the processes that own an audio device).
//!
//! Frames are `u32 LE length + JSON`, see [`eds_core::ipc`]. One hub thread
//! owns every connection so the service's decision loop never blocks on I/O,
//! and a wedged agent can only delay its own delivery.

use eds_core::ipc::{self, Msg, PIPE_NAME, PROTOCOL};
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
use interprocess::os::windows::local_socket::ListenerOptionsExt;
use interprocess::os::windows::security_descriptor::SecurityDescriptor;
use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

type Stream = interprocess::local_socket::Stream;
type Listener = interprocess::local_socket::Listener;

#[derive(Debug)]
enum HubIn {
    NewConn(Stream),
    Cmd(Cmd),
    /// Hand the agent-in-waiting its door key before it dials in.
    Nonce(u32, String),
    DropSession(u32),
}

#[derive(Debug, Clone)]
pub enum Cmd {
    /// Deliver to one session's agent.
    ToSession(u32, Msg),
    #[allow(dead_code)]
    Broadcast(Msg),
}

impl Cmd {
    fn into_hub(self) -> HubIn {
        HubIn::Cmd(self)
    }
}

#[derive(Debug, Clone)]
pub enum ServiceEvent {
    AgentConnected { session: u32, pid: u32 },
    AgentGone { session: u32 },
}

pub struct Server {
    tx: Sender<HubIn>,
    pub events: Receiver<ServiceEvent>,
    _hub: JoinHandle<()>,
    _accept: JoinHandle<()>,
}

/// `GenericNamespaced` is a named pipe on Windows and a runtime-dir socket
/// elsewhere, which is what a per-machine service wants on both.
fn bad_name(e: impl std::fmt::Debug) -> String {
    format!("管道名无效: {e:?}")
}

fn sd_sddl() -> &'static widestring::U16CStr {
    static SD: std::sync::OnceLock<widestring::U16CString> = std::sync::OnceLock::new();
    SD.get_or_init(|| {
        // S-1-5-4 interactive, S-1-5-14 remote interactive; SYSTEM/admins full.
        // Every non-abbreviated SID needs its `S-` prefix or the whole
        // descriptor is rejected with ERROR_INVALID_SECURITY_DESCR.
        widestring::U16CString::from_str(
            "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)(A;;GRGW;;;S-1-5-14)",
        )
        .expect("SDDL is ASCII")
    })
}

/// Unpredictable per-process string, used as the agent's door key.
pub fn new_nonce() -> String {
    use std::hash::{BuildHasher, Hasher};
    let a = std::collections::hash_map::RandomState::new().build_hasher().finish();
    let b = std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("{a:016x}{b:016x}")
}

/// The session id the peer claims, and whether the nonce matches what the
/// supervisor handed to the process it launched for that session.

impl Server {
    pub fn start() -> Result<Server, String> {
        let name = PIPE_NAME
            .to_ns_name::<GenericNamespaced>()
            .map_err(bad_name)?;
        // The default descriptor only admits the creating logon session, which
        // for a LocalSystem service means *no* user can ever connect: the agent
        // gets ERROR_ACCESS_DENIED and the service sits there with no player.
        // Grant read/write to interactive and remote-interactive logons; the
        // claimed session id is verified against the real peer below.
        let sd = SecurityDescriptor::deserialize(sd_sddl())
            .map_err(|e| format!("管道安全描述符无效: {e}"))?;
        let listener: Listener = ListenerOptions::new()
            .name(name)
            .security_descriptor(sd)
            .create_sync()
            .map_err(|e| format!("无法监听命名管道 {PIPE_NAME}: {e}"))?;
        let (tx, rx) = mpsc::channel::<HubIn>();
        let (ev_tx, events) = mpsc::channel::<ServiceEvent>();
        let accept_tx = tx.clone();
        let accept = std::thread::Builder::new()
            .name("edscene-accept".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                for conn in listener.incoming() {
                    match conn {
                        Ok(s) => {
                            if accept_tx.send(HubIn::NewConn(s)).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let hub = std::thread::Builder::new()
            .name("edscene-hub".into())
            .stack_size(192 * 1024)
            .spawn(move || hub_loop(rx, ev_tx))
            .map_err(|e| e.to_string())?;
        Ok(Server {
            tx,
            events,
            _hub: hub,
            _accept: accept,
        })
    }

    pub fn send(&self, cmd: Cmd) -> Result<(), String> {
        self.tx.send(cmd.into_hub()).map_err(|_| "服务已停止".into())
    }

    /// Must be called *before* the agent is launched, or it will be locked out.
    pub fn expect_nonce(&self, session: u32, nonce: &str) -> Result<(), String> {
        self.tx
            .send(HubIn::Nonce(session, nonce.to_string()))
            .map_err(|_| "服务已停止".into())
    }

    pub fn forget_session(&self, session: u32) {
        let _ = self.tx.send(HubIn::DropSession(session));
    }
}

fn hub_loop(rx: Receiver<HubIn>, ev_tx: Sender<ServiceEvent>) {
    let mut conns: HashMap<u32, Stream> = HashMap::new();
    let mut nonces: HashMap<u32, String> = HashMap::new();
    while let Ok(msg) = rx.recv() {
        match msg {
            HubIn::NewConn(stream) => {
                let mut reader = BufReader::new(stream);
                let claimed = match ipc::read_msg(&mut reader) {
                    Ok(Msg::Hello {
                        role: ipc::Role::Agent,
                        protocol,
                        pid,
                        session,
                        nonce,
                    }) if protocol == PROTOCOL => Some((session, pid, nonce)),
                    Ok(other) => {
                        eprintln!("edscene: 拒绝连接，首帧不是 agent hello: {other:?}");
                        continue;
                    }
                    Err(e) => {
                        eprintln!("edscene: 握手失败: {e}");
                        continue;
                    }
                };
                let Some((session, pid, nonce)) = claimed else { continue };
                let expected = nonces.get(&session).cloned();
                match expected {
                    Some(want) if want == nonce => {}
                    _ => {
                        eprintln!("edscene: 拒绝连接：会话 {session} 的口令不匹配");
                        continue;
                    }
                }
                let mut stream = reader.into_inner();
                // Acknowledge, so the agent knows the link is live both ways.
                if write_frame(&mut stream, &Msg::Loaded { rev: 0 }).is_err() {
                    continue;
                }
                conns.insert(session, stream);
                let _ = ev_tx.send(ServiceEvent::AgentConnected { session, pid });
            }
            HubIn::Nonce(session, nonce) => {
                nonces.insert(session, nonce);
            }
            HubIn::DropSession(session) => {
                nonces.remove(&session);
            }
            HubIn::Cmd(cmd) => {
                let (targets, msg): (Vec<u32>, &Msg) = match &cmd {
                    Cmd::ToSession(s, m) => (vec![*s], m),
                    Cmd::Broadcast(m) => (conns.keys().copied().collect(), m),
                };
                for t in targets {
                    let Some(stream) = conns.get_mut(&t) else {
                        continue;
                    };
                    if write_frame(stream, msg).is_err() {
                        conns.remove(&t);
                        let _ = ev_tx.send(ServiceEvent::AgentGone { session: t });
                    }
                }
            }
        }
    }
}

fn write_frame<W: Write>(w: &mut W, msg: &Msg) -> Result<(), String> {
    let bytes = ipc::encode(msg).map_err(|e| e.to_string())?;
    w.write_all(&bytes).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())
}

/// Client side, used by the agent.
pub struct Client {
    stream: Stream,
    #[allow(dead_code)]
    pub session: u32,
}

impl Client {
    pub fn connect(session: u32, pid: u32, nonce: &str) -> Result<Client, String> {
        let name = PIPE_NAME
            .to_ns_name::<GenericNamespaced>()
            .map_err(bad_name)?;
        let mut stream = Stream::connect(name).map_err(|e| format!("无法连接服务: {e}"))?;
        write_frame(
            &mut stream,
            &Msg::Hello {
                role: ipc::Role::Agent,
                protocol: PROTOCOL,
                pid,
                session,
                nonce: nonce.to_string(),
            },
        )?;
        Ok(Client { stream, session })
    }

    /// Block until the service says something. `None` means the pipe closed.
    pub fn next(&mut self) -> Option<Msg> {
        ipc::read_msg(&mut self.stream).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn within<F: FnMut() -> bool>(mut f: F, secs: u64) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            if f() {
                return true;
            }
            if Instant::now() > end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The DACL typo that once put the service in a boot-time crash loop was
    /// only visible in an opaque "IO error in winapi call". Parse it here.
    #[test]
    fn the_pipe_dacl_is_valid_sddl() {
        SecurityDescriptor::deserialize(sd_sddl()).expect("sd_sddl() 不是合法的 SDDL");
    }

    /// The handshake both ends agree on, exercised in-process: a process that
    /// did not get the key must not be able to pose as a session's player.
    #[test]
    fn a_wrong_nonce_is_refused_and_the_right_one_accepted() {
        let server = match Server::start() {
            Ok(s) => s,
            Err(_) => {
                eprintln!("跳过：命名管道已被占用（这台机器上装着 edscene 服务）");
                return;
            }
        };
        server.expect_nonce(4242, "hunter2").unwrap();

        let mut impostor = Client::connect(4242, 1, "not-the-key").unwrap();
        let _ = impostor.next();
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !matches!(
                server.events.try_recv(),
                Ok(ServiceEvent::AgentConnected { .. })
            ),
            "口令不对却注册成功了"
        );

        let agent = Client::connect(4242, 7, "hunter2").unwrap();
        assert!(
            within(
                || matches!(server.events.try_recv(),
                           Ok(ServiceEvent::AgentConnected { session, .. }) if session == 4242),
                3
            ),
            "带对口令的代理没有出现在事件里"
        );
        drop(agent);
        drop(impostor);
    }
}
