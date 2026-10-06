//! Platform-agnostic core for edscene: preset table, event state machine,
//! configuration model and sound-file resolution.
//!
//! Nothing in this crate touches the OS: providers feed [`Signal`]s into
//! [`Dispatcher`] with a caller-supplied monotonic timestamp, which keeps the
//! whole rule set unit-testable and lets the same logic run inside a Windows
//! service, a Linux system daemon and the `--simulate` harness.

pub mod config;
pub mod dispatcher;
pub mod event;
pub mod fx;
pub mod ipc;
pub mod presets;
pub mod resolve;

pub use config::{Config, Paths};
pub use dispatcher::{Dispatcher, Signal};
pub use event::EventKind;
pub use ipc::{Msg, Role, SeqGuard, PROTOCOL};
pub use presets::{EventDef, EVENTS};
pub use resolve::{Resolution, ResolveSource, SoundStatus};

pub const APP_ID: &str = "edscene";
pub const APP_NAME_ZH: &str = "集成工业播报终端";
pub const APP_NAME_EN: &str = "Integrated Industries Broadcast Terminal";
pub const CONFIG_VERSION: u32 = 1;
