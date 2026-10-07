//! Start the broadcast agent inside a user session from session 0.
//!
//! A service cannot play audio itself: the default render endpoint belongs to
//! the interactive user, not to LocalSystem. So the service duplicates the
//! session's primary token and launches the agent as that user on the real
//! desktop, which is also what lets the greeting land before the shell has
//! finished painting.

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::RemoteDesktop::WTSQueryUserToken;
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken, CREATE_UNICODE_ENVIRONMENT,
    DETACHED_PROCESS, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, DuplicateTokenEx, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    SE_TCB_NAME, SECURITY_IMPERSONATION_LEVEL, SecurityImpersonation, TOKEN_ACCESS_MASK,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_IMPERSONATE,
    TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_TYPE, TokenPrimary, LUID_AND_ATTRIBUTES,
};

/// LocalSystem already holds SeTcbPrivilege; an interactive `--agent` run does
/// not, and must not fail because of it.
unsafe fn enable_tcb_privilege() {
    let mut luid = LUID::default();
    if LookupPrivilegeValueW(None, SE_TCB_NAME, &mut luid).is_err() {
        return;
    }
    let mut token = HANDLE::default();
    if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES, &mut token)
        .is_err()
    {
        return;
    }
    let mut tp = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    let _ = AdjustTokenPrivileges(token, false, Some(&mut tp), 0, None, None);
    let _ = CloseHandle(token);
}

/// Safe wrapper: the service calls this from a session-0 thread.
pub fn spawn_agent_safe(session: u32, nonce: &str) -> Result<u32, String> {
    let args = format!("--agent --session {session} --agent-nonce {nonce}");
    unsafe { spawn_in_session(session, &args) }
}

/// The settings window, living in the notification area. The service starts it
/// next to the player so opening the GUI never means hunting for an exe.
pub fn spawn_tray_safe(session: u32) -> Result<u32, String> {
    unsafe { spawn_in_session(session, "--tray") }
}

/// Launch this exe with `args` inside `session`, on the interactive desktop.
unsafe fn spawn_in_session(session: u32, args: &str) -> Result<u32, String> {
    enable_tcb_privilege();
    let exe = std::env::current_exe().map_err(|e| format!("找不到自身路径: {e}"))?;
    let exe_w = wide(&exe.to_string_lossy());
    let mut desktop = wide(r"winsta0\Default");
    let mut cmdline = wide(&format!("\"{}\" {}", exe.display(), args));

    let mut user_token = HANDLE::default();
    WTSQueryUserToken(session, &mut user_token)
        .map_err(|e| format!("会话 {session} 还没有用户令牌（尚未登录）: {e}"))?;

    let mut primary = HANDLE::default();
    if DuplicateTokenEx(
        user_token,
        TOKEN_ACCESS_MASK(TOKEN_ASSIGN_PRIMARY.0 | TOKEN_DUPLICATE.0 | TOKEN_IMPERSONATE.0 | TOKEN_QUERY.0),
        None,
        SECURITY_IMPERSONATION_LEVEL(SecurityImpersonation.0),
        TOKEN_TYPE(TokenPrimary.0),
        &mut primary,
    )
    .is_err()
    {
        let _ = CloseHandle(user_token);
        return Err("无法复制用户令牌".into());
    }
    let _ = CloseHandle(user_token);

    let mut env: *mut core::ffi::c_void = core::ptr::null_mut();
    let have_env = CreateEnvironmentBlock(&mut env, Some(primary), false).is_ok();

    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop.as_mut_ptr()),
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    let flags = PROCESS_CREATION_FLAGS(
        CREATE_UNICODE_ENVIRONMENT.0 | DETACHED_PROCESS.0,
    );
    let created = CreateProcessAsUserW(
        Some(primary),
        PCWSTR(exe_w.as_ptr()),
        Some(PWSTR(cmdline.as_mut_ptr())),
        None,
        None,
        false,
        flags,
        if have_env { Some(env) } else { None },
        PCWSTR::null(),
        &si,
        &mut pi,
    );
    if have_env {
        let _ = DestroyEnvironmentBlock(env);
    }
    let _ = CloseHandle(primary);
    created.map_err(|e| format!("CreateProcessAsUserW 失败: {e}"))?;
    let pid = pi.dwProcessId;
    let _ = CloseHandle(pi.hProcess);
    let _ = CloseHandle(pi.hThread);
    Ok(pid)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
