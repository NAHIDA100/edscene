//! The four things only the Win32 API can do here: show a console, ask whether
//! we are elevated, hand ourselves to UAC, and write a Start Menu entry.

use std::path::Path;
use windows::core::{Interface, GUID, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, IPersistFile,
};
use windows::Win32::System::Console::{AllocConsole, SetConsoleCP, SetConsoleOutputCP};
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{ShellExecuteW, IShellLinkW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Shell and file picker CLSIDs are not exported by the `windows` crate.
const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_C000_000000000046);

/// UTF-16 with the terminator the API expects. The Vec has to outlive the call.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Double-clicked programs have no console; allocate one so the user can read
/// what happened. When Setup is started from a terminal this simply fails and
/// the existing console is kept.
pub fn attach_console() {
    unsafe {
        let _ = AllocConsole();
        // Without this the Chinese banner arrives as mojibake in a GBK console.
        let _ = SetConsoleOutputCP(65001);
        let _ = SetConsoleCP(65001);
    }
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = windows::Win32::Foundation::HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elev as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(token);
        ok && elev.TokenIsElevated != 0
    }
}

/// One UAC prompt, then this same binary continues as Administrator.
pub fn relaunch_elevated(args: &[String]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().to_string();
    let params = args
        .iter()
        .map(|a| {
            if a.contains(' ') && !a.starts_with('"') {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let (op, file, prm) = (wide("runas"), wide(&exe), wide(&params));
    let h = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(op.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(prm.as_ptr()),
            None,
            SW_SHOWNORMAL,
        )
    };
    match h.0 as i32 {
        0..=32 => Err(match h.0 as i32 {
            1223 => "已取消 UAC 授权，没有安装任何东西".into(),
            c => format!("无法请求管理员权限（ShellExecute 返回 {c}）"),
        }),
        _ => {
            println!("已请求管理员权限，正在弹出的窗口里继续。\n（这个窗口可以关了。）");
            Ok(())
        }
    }
}

/// A Start Menu entry pointing at the installed program, no COM ceremony left
/// uninitialised.
pub fn start_menu_shortcut(exe: &Path) -> Result<String, String> {
    let appdata = std::env::var("APPDATA").map_err(|_| "找不到 APPDATA".to_string())?;
    let dir = Path::new(&appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{dir:?}: {e}"))?;
    let lnk = dir.join("集成工业播报台.lnk");
    let lnk_s = lnk.to_string_lossy().to_string();
    let exe_s = exe.to_string_lossy().to_string();
    let work = exe
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    unsafe {
        // S_FALSE means COM was already up, which is fine; anything else is not.
        let co = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if co.is_err() && co.0 != 0x0000_0001 {
            return Err(format!("CoInitializeEx: {co:?}"));
        }
        let r = (|| -> windows::core::Result<()> {
            let link: IShellLinkW = CoCreateInstance(&CLSID_SHELL_LINK, None, CLSCTX_INPROC_SERVER)?;
            let (p, w, d, a) = (
                wide(&exe_s),
                wide(&work),
                wide("集成工业播报台 — 设置与试听"),
                wide(""),
            );
            link.SetPath(PCWSTR(p.as_ptr()))?;
            link.SetWorkingDirectory(PCWSTR(w.as_ptr()))?;
            link.SetDescription(PCWSTR(d.as_ptr()))?;
            link.SetArguments(PCWSTR(a.as_ptr()))?;
            let pf: IPersistFile = link.cast()?;
            let l = wide(&lnk_s);
            pf.Save(PCWSTR(l.as_ptr()), true)?;
            Ok(())
        })();
        CoUninitialize();
        r.map_err(|e| e.message().to_string())?;
    }
    Ok(lnk_s)
}
