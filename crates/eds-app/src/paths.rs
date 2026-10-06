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

/// Machine-wide location the service prefers when it can write there. Kept
/// separate from `user_paths` so a non-elevated UI can still run.
pub fn machine_paths() -> Option<Paths> {
    #[cfg(windows)]
    {
        let pd = env_or("PROGRAMDATA", &PathBuf::from("C:\\ProgramData"));
        let dir = pd.join("edscene");
        if std::fs::create_dir_all(&dir).is_ok() {
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

/// The file both the service and the UI actually use: the machine-wide one when
/// it already exists (an installed service owns it), otherwise the user one.
pub fn active_paths() -> Paths {
    if let Some(mp) = machine_paths() {
        if mp.config.exists() || mp.override_dir.exists() {
            return mp;
        }
    }
    user_paths()
}
