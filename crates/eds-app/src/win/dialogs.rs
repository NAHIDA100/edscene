//! Native file/folder picking and the blurred-wallpaper backdrop, so the UI
//! never has to shell out or embed a webview.

use std::path::PathBuf;
use windows::core::{w, PCWSTR};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
    GetDeviceCaps, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DESKTOPHORZRES,
    DESKTOPVERTRES, DIB_RGB_COLORS, HBITMAP, SRCCOPY,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{
    IFileOpenDialog, FileOpenDialog, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETDESKWALLPAPER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};

fn com_init() {
    unsafe {
        // Already-initialized and a different threading model are both fine here.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

fn show_dialog(folder: bool, title: &str) -> Option<PathBuf> {
    com_init();
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let mut opts = dlg.GetOptions().ok()?;
        if folder {
            opts |= FOS_PICKFOLDERS;
        }
        dlg.SetOptions(opts).ok()?;
        let t = wide(title);
        dlg.SetTitle(PCWSTR(t.as_ptr())).ok()?;
        dlg.Show(None).ok()?;
        let item = dlg.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = name.to_string().ok()?;
        windows::Win32::System::Com::CoTaskMemFree(Some(name.as_ptr() as *const _));
        Some(PathBuf::from(s))
    }
}

pub fn pick_file(title: &str) -> Option<PathBuf> {
    show_dialog(false, title)
}

pub fn pick_folder(title: &str) -> Option<PathBuf> {
    show_dialog(true, title)
}

/// Reveal a folder (or a file's parent) in Explorer.
pub fn reveal(path: &std::path::Path) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let target = wide(&format!("{}\\", path.display()));
    unsafe {
        let _ = ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(target.as_ptr()),
            PCWSTR::null(),
            None,
            SW_SHOWNORMAL,
        );
    }
}

/// The desktop wallpaper, blurred.
///
/// Reading the wallpaper *file* is what makes this read as a backdrop. Grabbing
/// the screen instead — which is what this used to do — turns the window into a
/// smear of whatever happened to be behind it, and users called it 泛红.
pub fn blurred_wallpaper() -> Option<slint::Image> {
    wallpaper_files()
        .iter()
        .find_map(|p| crate::backdrop::soften_path(p))
        .or_else(screen_grab)
}

/// Where the shell keeps the current wallpaper. Spotlight / slideshow
/// wallpapers are only ever reachable through the transcoded copy, which has no
/// extension at all — hence the decoder sniffing the bytes, not the name.
fn wallpaper_files() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let mut buf = [0u16; 520];
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            buf.len() as u32,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..end]);
    if !s.is_empty() {
        v.push(PathBuf::from(s));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let themes = PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Themes");
        v.push(themes.join("TranscodedWallpaper"));
        v.push(themes.join("TranscodedWallpaper.jpg"));
    }
    v
}

/// Last resort: grab the desktop, shrink it hard and let Slint's bilinear
/// stretch do the rest. `None` on any failure, which the UI treats as 纯色.
fn screen_grab() -> Option<slint::Image> {
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return None;
        }
        let w = GetDeviceCaps(Some(screen), DESKTOPHORZRES);
        let h = GetDeviceCaps(Some(screen), DESKTOPVERTRES);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp: HBITMAP = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), 0, 0, SRCCOPY);
        let mut pixels = vec![0u8; (w as usize * h as usize * 4).max(1)];
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let got = GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            Some(pixels.as_mut_ptr() as *mut core::ffi::c_void),
            &mut bi,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if ok.is_err() || got == 0 {
            return None;
        }
        crate::backdrop::from_bgra(&pixels, w as usize, h as usize)
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

