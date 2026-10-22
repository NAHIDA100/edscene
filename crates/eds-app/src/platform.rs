//! The few things the UI needs from the OS that are not events: autostart
//! state, a backdrop image, and moving a borderless window.

/// Whether the header strip has to be dragged by us. With a native frame
/// Windows does it, and an overlay TouchArea there would only swallow clicks.
pub fn native_drag(native_frame: bool) -> bool {
    cfg!(windows) && !native_frame
}

/// How long the machine has been up. The dispatcher needs it to tell a logon
/// that means "just booted" from one that means "somebody sat down again", even
/// when this process started hours after the boot.
#[cfg(windows)]
pub fn uptime_ms() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

#[cfg(not(windows))]
pub fn uptime_ms() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split('.').next()?.parse::<f64>().ok())
        .map(|s| (s * 1_000.0) as u64)
        .unwrap_or(0)
}

#[cfg(windows)]
pub fn set_autostart(on: bool) -> Result<(), String> {
    crate::win::service::set_autostart(on)
}

#[cfg(windows)]
pub fn service_installed() -> bool {
    crate::win::service::installed()
}

#[cfg(windows)]
/// The service can only be registered from an elevated process, so hand the
/// whole job back to UAC instead of telling the user to open a terminal.
pub fn elevate_for_autostart() -> Result<(), String> {
    crate::win::service::elevate("--install-service")
}

/// Where to look when an autostart change comes back without a reason: both the
/// installer and the UAC child have no console this output could live in.
#[cfg(windows)]
pub fn autostart_log_hint() -> String {
    crate::win::service::install_log_path().display().to_string()
}

#[cfg(not(windows))]
pub fn autostart_log_hint() -> String {
    "journalctl --user -u edscene".into()
}

#[cfg(windows)]
pub fn blurred_wallpaper() -> Option<slint::Image> {
    crate::win::dialogs::blurred_wallpaper()
}

#[cfg(not(windows))]
pub fn set_autostart(on: bool) -> Result<(), String> {
    crate::linux::set_autostart(on)
}

#[cfg(not(windows))]
pub fn elevate_for_autostart() -> Result<(), String> {
    Err("用户级自启不需要管理员权限".into())
}

#[cfg(not(windows))]
pub fn service_installed() -> bool {
    crate::linux::unit_installed()
}

#[cfg(not(windows))]
pub fn blurred_wallpaper() -> Option<slint::Image> {
    crate::linux::blurred_wallpaper()
}

/// The backdrop, in order of preference: a picture the user picked, the one
/// that shipped beside the exe, then the desktop's own wallpaper.
pub fn backdrop(path: &str) -> Option<slint::Image> {
    if !path.is_empty() {
        if let Some(img) = crate::backdrop::soften_path(std::path::Path::new(path)) {
            return Some(img);
        }
    }
    if let Some(p) = crate::paths::shipped_backdrop() {
        if let Some(img) = crate::backdrop::soften_path(&p) {
            return Some(img);
        }
    }
    blurred_wallpaper()
}

// ------------------------------------------------------------------- dragging

#[cfg(windows)]
pub mod drag {
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetCursorPos, GetWindowRect, SetWindowPos, WindowFromPoint, GA_ROOT,
        SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };

    /// Where the cursor grabbed the window, in physical screen pixels. Keeping
    /// the offset absolute is what stops the window from lagging behind the
    /// mouse when we move it ourselves.
    pub struct Grab {
        hwnd: HWND,
        dx: i32,
        dy: i32,
    }

    impl Grab {
        /// What we grabbed, for the log: a bare `0x…` is enough to tell a wrong
        /// window from a failed move.
        pub fn describe(&self) -> String {
            format!("hwnd={:p}", self.hwnd.0)
        }
    }

    fn cursor() -> Option<POINT> {
        let mut p = POINT::default();
        unsafe { GetCursorPos(&mut p).ok().map(|_| p) }
    }

    pub fn begin() -> Result<Grab, String> {
        let p = cursor().ok_or("GetCursorPos 失败")?;
        let hit = unsafe { WindowFromPoint(p) };
        if hit.is_invalid() {
            return Err("光标底下没有窗口".into());
        }
        // Slint keeps helper windows around; move the frame that owns them.
        let hwnd = unsafe { GetAncestor(hit, GA_ROOT) };
        if hwnd.is_invalid() {
            return Err("找不到顶层窗口".into());
        }
        let mut r = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut r).map_err(|e| format!("GetWindowRect: {e}"))? };
        Ok(Grab { hwnd, dx: p.x - r.left, dy: p.y - r.top })
    }

    /// Move the window so the point under the finger stays under the finger:
    /// both sides read absolute screen pixels, so it neither lags nor jumps.
    pub fn apply(g: &Grab) {
        let Some(p) = cursor() else { return };
        unsafe {
            let _ = SetWindowPos(
                g.hwnd,
                None,
                p.x - g.dx,
                p.y - g.dy,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

#[cfg(not(windows))]
pub mod drag {
    pub struct Grab;
    impl Grab {
        pub fn describe(&self) -> String {
            "n/a".into()
        }
    }
    pub fn begin() -> Result<Grab, String> {
        Err("这个平台交给 WindowMoveArea".into())
    }
    pub fn apply(_: &Grab) {}
}
