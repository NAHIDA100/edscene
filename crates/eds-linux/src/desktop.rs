//! Linux desktop integration that does not need D-Bus: the systemd user unit,
//! the XDG autostart entry, and how to get a picture of the desktop.
//!
//! Everything here is pure string/path work so it can be type-checked (and its
//! content unit-tested) from any host; only `Command` spawning lives in the
//! binary.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const UNIT_NAME: &str = "edscene.service";
pub const AUTOSTART_NAME: &str = "io.github.edscene.desktop";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(fallback))
}

pub fn user_unit_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("systemd").join("user")
}

pub fn autostart_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("autostart")
}

pub fn user_config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("edscene")
}

/// The agent runs inside the graphical session: it needs the session bus for
/// the screensaver source and the audio socket for playback.
pub fn unit_contents(exe: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=edscene broadcast agent (per-session voice announcements)\n\
         PartOf=graphical-session.target\n\
         After=graphical-session.target pipewire.socket pulseaudio.socket\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} --agent\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         Nice=10\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
        exe.display()
    )
}

/// For desktops without systemd (or a user bus we cannot reach): a plain XDG
/// autostart entry. `XDG_CURRENT_DESKTOP` is left alone on purpose so the file
/// works on GNOME, KDE, and bare WMs alike.
pub fn autostart_contents(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=edscene 播报代理\n\
         Comment=集成工业播报终端：登录、解锁与电源事件的语音播报\n\
         Exec={} --agent\n\
         Icon=edscene\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exe.display()
    )
}

pub fn unit_installed() -> bool {
    user_unit_dir().join(UNIT_NAME).exists() || autostart_dir().join(AUTOSTART_NAME).exists()
}

/// Write (or remove) both files. Returns what changed so the caller can report
/// it; enabling the unit is a separate step because it shells out.
pub fn write_launch_files(exe: &Path, enable: bool) -> Result<Vec<PathBuf>, String> {
    let mut touched = Vec::new();
    if enable {
        std::fs::create_dir_all(user_unit_dir()).map_err(|e| e.to_string())?;
        let u = user_unit_dir().join(UNIT_NAME);
        std::fs::write(&u, unit_contents(exe)).map_err(|e| e.to_string())?;
        touched.push(u);
        std::fs::create_dir_all(autostart_dir()).map_err(|e| e.to_string())?;
        let a = autostart_dir().join(AUTOSTART_NAME);
        std::fs::write(&a, autostart_contents(exe)).map_err(|e| e.to_string())?;
        touched.push(a);
    } else {
        for p in [user_unit_dir().join(UNIT_NAME), autostart_dir().join(AUTOSTART_NAME)] {
            match std::fs::remove_file(&p) {
                Ok(()) => touched.push(p),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("无法删除 {}: {e}", p.display())),
            }
        }
    }
    Ok(touched)
}

/// systemctl arguments for the two states.
pub fn systemctl_args(enable: bool) -> Vec<OsString> {
    let mut v = vec![OsString::from("--user")];
    v.push(OsString::from(if enable { "enable" } else { "disable" }));
    v.push(OsString::from(UNIT_NAME));
    if enable {
        v.push(OsString::from("--now"));
    }
    v
}

/// Where a desktop says its wallpaper lives. GNOME/KDE expose it through a
/// setting; everything else falls back to the usual picture folders.
pub fn wallpaper_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(u) = gsettings_picture_uri() {
        out.push(u);
    }
    if let Ok(dir) = std::env::var("XDG_PICTURES_DIR") {
        out.push(PathBuf::from(dir));
    }
    out.push(home().join(".config").join("hypr").join("wallpaper.jpg"));
    out.push(home().join("Pictures").join("wallpaper.jpg"));
    out.push(home().join("Pictures").join("wallpaper.png"));
    out.retain(|p| p.is_file());
    out
}

fn gsettings_picture_uri() -> Option<PathBuf> {
    for key in ["picture-uri-dark", "picture-uri"] {
        let ok = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.background", key])
            .output()
            .ok()?;
        if !ok.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&ok.stdout);
        let uri = text.trim().trim_matches('\'').trim_matches('"');
        let path = uri.strip_prefix("file://")?;
        let p = PathBuf::from(percent_decode(path));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Screenshot tools that can actually see the desktop, in the order they are
/// likely to exist. Wayland cannot be grabbed through X11 calls, hence grim.
/// Each entry is a full argv; the tool name is `argv[0]`.
pub fn screenshot_argv(wayland: bool) -> Vec<&'static [&'static str]> {
    if wayland {
        vec![&["grim", "-"]]
    } else {
        vec![
            &["import", "-window", "root", "png:-"],
            &["scrot", "--overwrite", "-"],
            &["gnome-screenshot", "-f", "/dev/stdout"],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_runs_the_agent_inside_the_graphical_session() {
        let t = unit_contents(Path::new("/usr/bin/edscene"));
        assert!(t.contains("ExecStart=/usr/bin/edscene --agent"), "unit was:
{t}");
        assert!(t.contains("PartOf=graphical-session.target"));
        assert!(t.contains("WantedBy=graphical-session.target"));
        assert!(t.contains("Restart=on-failure"));
        assert!(t.contains("Nice=10"), "a background greeter should not out-prioritise the shell");
    }

    #[test]
    fn the_autostart_entry_is_valid_desktop_spec() {
        let t = autostart_contents(Path::new("/opt/edscene/edscene"));
        assert!(t.starts_with("[Desktop Entry]"));
        assert!(t.contains("Exec=/opt/edscene/edscene --agent"));
        assert!(t.contains("Terminal=false"));
    }

    #[test]
    fn systemctl_args_match_the_toggle() {
        let on = systemctl_args(true);
        assert_eq!(on[1], "enable");
        assert!(on.contains(&OsString::from("--now")));
        assert!(!systemctl_args(false).contains(&OsString::from("--now")));
    }

    #[test]
    fn percent_decoding_handles_spaces_in_wallpaper_paths() {
        assert_eq!(percent_decode("/home/a%20b/w.png"), "/home/a b/w.png");
        assert_eq!(percent_decode("/plain.png"), "/plain.png");
    }

    #[test]
    fn write_and_remove_round_trips_in_a_temp_tree() {
        let root = std::env::temp_dir().join(format!("edscene-dt-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &root);
        let exe = Path::new("/usr/local/bin/edscene");
        let touched = write_launch_files(exe, true).unwrap();
        assert_eq!(touched.len(), 2);
        assert!(unit_installed());
        let again = write_launch_files(exe, false).unwrap();
        assert_eq!(again.len(), 2);
        assert!(!unit_installed());
        let _ = std::fs::remove_dir_all(&root);
    }
}
