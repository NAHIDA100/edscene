//! Named-pipe link between the service (event originator) and the per-session
//! agents (the processes that own an audio device).
//!
//! Frames are `u32 LE length + JSON`, see [`eds_core::ipc`]. One hub thread
//! owns every connection so the service's decision loop never blocks on I/O,
//! and a wedged agent can only delay its own delivery.

use eds_core::ipc::{self, Msg, PIPE_NAME, PROTOCOL};
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, ToNsName};
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
}

#[derive(Debug, Clone)]
pub enum Cmd {
    /// Deliver to one session's agent.
    ToSession(u32, Msg),
    #[allow(dead_code)]
    Broadcast(Msg),
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

impl Server {
    pub fn start() -> Result<Server, String> {
        let name = PIPE_NAME
            .to_ns_name::<GenericNamespaced>()
            .map_err(bad_name)?;
        let listener: Listener = ListenerOptions::new()
            .name(name)
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
}

impl Cmd {
    fn into_hub(self) -> HubIn {
        HubIn::Cmd(self)
    }
}

fn hub_loop(rx: Receiver<HubIn>, ev_tx: Sender<ServiceEvent>) {
    let mut conns: HashMap<u32, Stream> = HashMap::new();
    while let Ok(msg) = rx.recv() {
        match msg {
            HubIn::NewConn(stream) => {
                let mut reader = BufReader::new(stream);
                let (session, pid) = match ipc::read_msg(&mut reader) {
                    Ok(Msg::Hello {
                        role: ipc::Role::Agent,
                        protocol,
                        pid,
                        session,
                    }) if protocol == PROTOCOL => (session, pid),
                    Ok(other) => {
                        eprintln!("edscene: 拒绝连接，首帧不是 agent hello: {other:?}");
                        continue;
                    }
                    Err(e) => {
                        eprintln!("edscene: 握手失败: {e}");
                        continue;
                    }
                };
                let mut stream = reader.into_inner();
                // Acknowledge, so the agent knows the link is live both ways.
                if write_frame(&mut stream, &Msg::Loaded { rev: 0 }).is_err() {
                    continue;
                }
                conns.insert(session, stream);
                let _ = ev_tx.send(ServiceEvent::AgentConnected { session, pid });
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
    pub fn connect(session: u32, pid: u32) -> Result<Client, String> {
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
            },
        )?;
        Ok(Client { stream, session })
    }

    /// Block until the service says something. `None` means the pipe closed.
    pub fn next(&mut self) -> Option<Msg> {
        ipc::read_msg(&mut self.stream).ok()
    }
}
