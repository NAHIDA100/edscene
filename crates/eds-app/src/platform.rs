//! The few things the UI needs from the OS that are not events: autostart
//! state, and a blurred desktop for the backdrop.

#[cfg(windows)]
pub fn set_autostart(on: bool) -> Result<(), String> {
    crate::win::service::set_autostart(on)
}

#[cfg(windows)]
pub fn service_installed() -> bool {
    crate::win::service::installed()
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
pub fn service_installed() -> bool {
    crate::linux::unit_installed()
}

#[cfg(not(windows))]
pub fn blurred_wallpaper() -> Option<slint::Image> {
    crate::linux::blurred_wallpaper()
}
