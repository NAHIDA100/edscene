//! The few things the UI needs from the OS that are not events: autostart
//! state, a backdrop image, and moving a borderless window.

/// Whether the header strip must be dragged by us. Slint's `WindowMoveArea`
/// does nothing for a frameless transparent window on Windows, so there we
/// track the cursor ourselves; elsewhere its implementation is left in place.
pub fn native_drag() -> bool {
    cfg!(windows)
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

/// The backdrop: a picture the user picked, else the desktop's own wallpaper.
pub fn backdrop(path: &str) -> Option<slint::Image> {
    if !path.is_empty() {
        if let Some(img) = crate::backdrop::soften_path(std::path::Path::new(path)) {
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
    /// the offset in absolute coordinates is what stops the window from lagging
    /// half a step behind the mouse.
    pub struct Grab {
        hwnd: HWND,
        dx: i32,
        dy: i32,
    }

    fn cursor() -> Option<POINT> {
        let mut p = POINT::default();
        unsafe { GetCursorPos(&mut p).ok().map(|_| p) }
    }

    pub fn begin() -> Option<Grab> {
        let p = cursor()?;
        let hit = unsafe { WindowFromPoint(p) };
        if hit.is_invalid() {
            return None;
        }
        // Slint keeps hidden helper windows around; move the frame that owns
        // them, not whatever piece of it happened to be under the cursor.
        let hwnd = unsafe { GetAncestor(hit, GA_ROOT) };
        if hwnd.is_invalid() {
            return None;
        }
        let mut r = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut r).ok()? };
        Some(Grab {
            hwnd,
            dx: p.x - r.left,
            dy: p.y - r.top,
        })
    }

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
    pub struct Grab {}
    pub fn begin() -> Option<Grab> {
        None
    }
    pub fn apply(_: &Grab) {}
}
