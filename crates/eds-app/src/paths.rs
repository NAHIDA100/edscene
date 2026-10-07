//! Where edscene keeps its own files. Hand-rolled instead of pulling in the
//! `directories` crate: three environment variables and a fallback each.
use eds_core::config::Paths;
use std::path::PathBuf;

fn env_or(key: &str, fallback: &PathBuf) -> PathBuf {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback.clone())
}

fn home() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"))
    }
}

/// Per-user state. The machine-wide config lives here too: the service and the
/// UI must agree on one file, and a per-user file is the only location both a
/// LocalSystem service and a normal desktop session can write without extra
/// ACL work.
pub fn user_paths() -> Paths {
    #[cfg(windows)]
    let base = env_or(
        "APPDATA",
        &home().join("AppData").join("Roaming"),
    );
    #[cfg(not(windows))]
    let base = env_or("XDG_CONFIG_HOME", &home().join(".config"));
    #[cfg(windows)]
    let data = env_or(
        "LOCALAPPDATA",
        &home().join("AppData").join("Local"),
    );
    #[cfg(not(windows))]
    let data = env_or("XDG_STATE_HOME", &home().join(".local").join("state"));

    Paths {
        config: base.join("edscene").join("config.toml"),
        override_dir: base.join("edscene").join("sounds"),
        state_dir: data.join("edscene"),
    }
}

/// Machine-wide location, *if it already exists*. Pure probe: a read-only
/// question must not create `C:\ProgramData\edscene` just because somebody ran
/// `--selftest` without elevation.
pub fn machine_paths() -> Option<Paths> {
    #[cfg(windows)]
    {
        let dir = env_or("PROGRAMDATA", &PathBuf::from("C:\\ProgramData")).join("edscene");
        if dir.is_dir() {
            return Some(Paths {
                config: dir.join("config.toml"),
                override_dir: dir.join("sounds"),
                state_dir: dir.join("state"),
            });
        }
        None
    }
    #[cfg(not(windows))]
    None
}

/// Create the machine-wide tree. Only the service (LocalSystem) and
/// `--install-service` (elevated) may call this.
pub fn ensure_machine_paths() -> Option<Paths> {
    #[cfg(windows)]
    {
        let mp = machine_paths().unwrap_or_else(|| {
            let dir = env_or("PROGRAMDATA", &PathBuf::from("C:\\ProgramData")).join("edscene");
            Paths {
                config: dir.join("config.toml"),
                override_dir: dir.join("sounds"),
                state_dir: dir.join("state"),
            }
        });
        std::fs::create_dir_all(&mp.state_dir).ok()?;
        Some(mp)
    }
    #[cfg(not(windows))]
    None
}

/// Files that ship *next to the executable* in a release archive. This
/// repository carries no audio and no artwork, so these only exist for people
/// who downloaded a build — which is exactly why they are a default, never a
/// requirement.
fn exe_dir() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.to_path_buf())
}

/// `sounds/` beside the exe, if it is there.
pub fn shipped_sounds() -> Option<PathBuf> {
    let dir = exe_dir()?.join("sounds");
    dir.is_dir().then_some(dir)
}

/// `background.jpg` / `.png` beside the exe, if it is there.
pub fn shipped_backdrop() -> Option<PathBuf> {
    let dir = exe_dir()?;
    ["background.jpg", "background.jpeg", "background.png"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
}

/// The file both the service and the UI actually use: the machine-wide one as
/// soon as an installed service owns the directory.
///
/// A user-side process (UI or agent) also *seeds* it: the service can read a
/// file we create, but a file created by LocalSystem could never be edited by
/// the settings window again, so the copy has to happen from here.
///
/// The one thing a user-side process must never do is write into the machine
/// state directory: `events.ndjson` there belongs to the service, and appending
/// to it as a normal user fails silently. Every log we write stays per-user.
pub fn active_paths() -> Paths {
    let user = user_paths();
    let Some(mut mp) = machine_paths() else {
        return user;
    };
    if !mp.config.exists() {
        if user.config.exists() {
            if let Some(dir) = mp.config.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::copy(&user.config, &mp.config);
        }
    }
    mp.state_dir = user.state_dir;
    mp
}
