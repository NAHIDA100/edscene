#![cfg(windows)]

use std::cell::Cell;
use std::mem::size_of;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::OnceLock;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, SetLastError, ERROR_SUCCESS, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP,
    NIM_ADD, NIM_DELETE, NIM_SETFOCUS, NIM_SETVERSION, NINF_KEY, NIN_SELECT, NOTIFYICONDATAW,
    NOTIFYICONIDENTIFIER, NOTIFYICON_VERSION_4, NOTIFY_ICON_MESSAGE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DestroyWindow, EndMenu, GetCursorPos, GetWindowLongPtrW,
    KillTimer, PostMessageW, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow,
    SetMenuDefaultItem, SetTimer, SetWindowLongPtrW, TrackPopupMenu, GWLP_USERDATA, HICON,
    HMENU, ICONINFO, MF_STRING, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU, WM_LBUTTONUP, WM_NCDESTROY, WM_NULL,
    WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

const CLASS_NAME: PCWSTR = w!("edscene.native.tray");
const ICON_PNG: &[u8] = include_bytes!("../../ui/icon.png");
const ICON_ID: u32 = 1;
const RETRY_TIMER: usize = 1;
const RETRY_MS: u32 = 2_000;
const WM_TRAY: u32 = WM_APP + 0x47;
const OPEN: u32 = 1;
const QUIT: u32 = 2;
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;

type Notify = Box<dyn Fn(NOTIFY_ICON_MESSAGE, &NOTIFYICONDATAW) -> bool>;
type Report = Box<dyn Fn(&str, &str)>;

pub struct Tray {
    state: Box<Rc<State>>,
}

struct State {
    hwnd: Cell<HWND>,
    icon: Icon,
    menu: Menu,
    taskbar_created: u32,
    on_open: Box<dyn Fn()>,
    on_quit: Box<dyn Fn()>,
    notify: Notify,
    report: Report,
    registered: Cell<bool>,
    version4: Cell<bool>,
    timer: Cell<bool>,
    waiting: Cell<bool>,
    closed: Cell<bool>,
    registering: Cell<bool>,
    restart_pending: Cell<bool>,
    menu_open: Cell<bool>,
    invoking: Cell<bool>,
    error_logged: Cell<bool>,
}

impl Tray {
    pub fn new(
        on_open: impl Fn() + 'static,
        on_quit: impl Fn() + 'static,
    ) -> Result<Self, String> {
        let result = Self::with_hooks(
            on_open,
            on_quit,
            Box::new(|operation, data| unsafe { Shell_NotifyIconW(operation, data).as_bool() }),
            Box::new(diagnostic),
        );
        if let Err(error) = &result {
            diagnostic("tray-error", error);
        }
        result
    }

    fn with_hooks(
        on_open: impl Fn() + 'static,
        on_quit: impl Fn() + 'static,
        notify: Notify,
        report: Report,
    ) -> Result<Self, String> {
        ensure_window_class()?;
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        if taskbar_created == 0 {
            return Err(last_error("RegisterWindowMessageW"));
        }
        let instance = unsafe { GetModuleHandleW(None) }
            .map_err(|error| format!("GetModuleHandleW: {error}"))?;
        let tray = Self {
            state: Box::new(Rc::new(State {
                hwnd: Cell::new(HWND::default()),
                icon: create_icon()?,
                menu: create_menu()?,
                taskbar_created,
                on_open: Box::new(on_open),
                on_quit: Box::new(on_quit),
                notify,
                report,
                registered: Cell::new(false),
                version4: Cell::new(false),
                timer: Cell::new(false),
                waiting: Cell::new(false),
                closed: Cell::new(false),
                registering: Cell::new(false),
                restart_pending: Cell::new(false),
                menu_open: Cell::new(false),
                invoking: Cell::new(false),
                error_logged: Cell::new(false),
            })),
        };
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                CLASS_NAME,
                w!("edscene tray"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }
        .map_err(|error| format!("CreateWindowExW: {error}"))?;
        tray.state.hwnd.set(hwnd);
        unsafe {
            SetLastError(ERROR_SUCCESS);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&*tray.state as *const Rc<State>) as _);
            if GetLastError() != ERROR_SUCCESS {
                return Err(last_error("SetWindowLongPtrW"));
            }
        }
        tray.state.try_register()?;
        Ok(tray)
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let hwnd = self.state.hwnd.get();
        if !hwnd.is_invalid() {
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            self.state.stop();
            self.state.hwnd.set(HWND::default());
            if let Err(error) = unsafe { DestroyWindow(hwnd) } {
                self.state.error(&format!("DestroyWindow: {error}"));
            }
        }
    }
}

impl State {
    fn identity(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd.get(),
            uID: ICON_ID,
            ..Default::default()
        }
    }

    fn try_register(&self) -> Result<(), String> {
        if self.closed.get() || self.registered.get() || self.registering.replace(true) {
            return Ok(());
        }
        let _registering = ResetFlag(&self.registering);
        self.restart_pending.set(false);
        let mut tip = [0u16; 128];
        for (slot, ch) in tip.iter_mut().zip("集成工业播报终端".encode_utf16()) {
            *slot = ch;
        }
        let data = NOTIFYICONDATAW {
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: WM_TRAY,
            hIcon: self.icon.0,
            szTip: tip,
            ..self.identity()
        };
        let added = (self.notify)(NIM_ADD, &data);
        let version4 = if added && !self.closed.get() && !self.restart_pending.get() {
            let mut version = self.identity();
            version.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            (self.notify)(NIM_SETVERSION, &version)
        } else {
            false
        };
        if self.closed.get() || self.restart_pending.get() {
            if added {
                (self.notify)(NIM_DELETE, &data);
            }
            if !self.closed.get() {
                self.start_retry()?;
            }
            return Ok(());
        }
        if added {
            self.registered.set(true);
            self.version4.set(version4);
            self.waiting.set(false);
            self.stop_retry();
            (self.report)("tray-ready", "通知区域图标已注册");
            if version4 {
                self.error_logged.set(false);
            } else {
                self.error("NIM_SETVERSION 失败，使用兼容鼠标通知");
            }
        } else {
            if !self.waiting.replace(true) {
                (self.report)("tray-wait", "NIM_ADD 失败，等待 Explorer，每 2 秒重试");
            }
            self.start_retry()?;
        }
        Ok(())
    }

    fn taskbar_created(&self) {
        if self.closed.get() {
            return;
        }
        self.registered.set(false);
        self.version4.set(false);
        self.restart_pending.set(true);
        if let Err(error) = self.try_register() {
            self.error(&error);
        }
    }

    fn start_retry(&self) -> Result<(), String> {
        if !self.timer.get() {
            if unsafe { SetTimer(Some(self.hwnd.get()), RETRY_TIMER, RETRY_MS, None) } == 0 {
                return Err(last_error("SetTimer"));
            }
            self.timer.set(true);
        }
        Ok(())
    }

    fn stop_retry(&self) {
        if self.timer.replace(false) {
            let _ = unsafe { KillTimer(Some(self.hwnd.get()), RETRY_TIMER) };
        }
    }

    fn stop(&self) {
        if self.closed.replace(true) {
            return;
        }
        self.stop_retry();
        self.registered.set(false);
        self.version4.set(false);
        if self.menu_open.get() {
            let _ = unsafe { EndMenu() };
        }
        (self.notify)(NIM_DELETE, &self.identity());
    }

    fn error(&self, error: &str) {
        if !self.error_logged.replace(true) {
            (self.report)("tray-error", error);
        }
    }

    fn invoke(&self, command: u32) {
        if self.closed.get() || self.invoking.replace(true) {
            return;
        }
        let _invoking = ResetFlag(&self.invoking);
        let result = catch_unwind(AssertUnwindSafe(|| match command {
            OPEN => (self.on_open)(),
            QUIT => (self.on_quit)(),
            _ => {}
        }));
        if let Err(payload) = result {
            std::mem::forget(payload);
            self.error("托盘操作回调发生 panic");
        }
    }

    fn menu_position(&self, position: Option<POINT>) -> Result<POINT, String> {
        if let Some(point) = position.filter(|point| point.x != -1 || point.y != -1) {
            return Ok(point);
        }
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.hwnd.get(),
            uID: ICON_ID,
            ..Default::default()
        };
        if let Ok(rect) = unsafe { Shell_NotifyIconGetRect(&identifier) } {
            return Ok(POINT {
                x: rect.left + (rect.right - rect.left) / 2,
                y: rect.top,
            });
        }
        let mut point = POINT::default();
        unsafe { GetCursorPos(&mut point) }
            .map_err(|error| format!("GetCursorPos: {error}"))?;
        Ok(point)
    }

    fn popup_menu(&self, position: Option<POINT>) -> Result<(), String> {
        if self.closed.get() || self.menu_open.replace(true) {
            return Ok(());
        }
        let _menu_open = ResetFlag(&self.menu_open);
        let point = self.menu_position(position)?;
        let hwnd = self.hwnd.get();
        let _ = unsafe { SetForegroundWindow(hwnd) };
        if self.closed.get() {
            return Ok(());
        }
        let command = unsafe {
            TrackPopupMenu(
                self.menu.0,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_BOTTOMALIGN,
                point.x,
                point.y,
                None,
                hwnd,
                None,
            )
        };
        if !self.closed.get() {
            let _ = unsafe { PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) };
            (self.notify)(NIM_SETFOCUS, &self.identity());
            self.invoke(command.0 as u32);
        }
        Ok(())
    }
}

struct ResetFlag<'a>(&'a Cell<bool>);

impl Drop for ResetFlag<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

#[derive(Debug, PartialEq)]
enum Action {
    Open,
    Menu(Option<POINT>),
}

fn point_from_param(value: isize) -> POINT {
    POINT {
        x: (value as u16 as i16) as i32,
        y: ((value as u32 >> 16) as u16 as i16) as i32,
    }
}

fn tray_action(version4: bool, wp: WPARAM, lp: LPARAM) -> Option<Action> {
    let icon_id = if version4 {
        (lp.0 as u32 >> 16) & 0xffff
    } else {
        wp.0 as u32
    };
    if icon_id != ICON_ID {
        return None;
    }
    match lp.0 as u32 & 0xffff {
        NIN_SELECT | NIN_KEYSELECT if version4 => Some(Action::Open),
        WM_LBUTTONUP if !version4 => Some(Action::Open),
        WM_CONTEXTMENU => Some(Action::Menu(version4.then(|| point_from_param(wp.0 as isize)))),
        WM_RBUTTONUP if !version4 => Some(Action::Menu(None)),
        _ => None,
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match catch_unwind(AssertUnwindSafe(|| dispatch_message(hwnd, msg, wp, lp))) {
        Ok(result) => result,
        Err(payload) => {
            std::mem::forget(payload);
            diagnostic("tray-error", "托盘窗口消息处理发生 panic");
            LRESULT(0)
        }
    }
}

fn dispatch_message(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Rc<State>;
    if pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    // 回调和原生菜单都可能重入并释放 Tray，因此消息处理期间另持一份状态所有权。
    let state = unsafe { Rc::clone(&*pointer) };
    if msg == WM_NCDESTROY {
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        state.stop();
        state.hwnd.set(HWND::default());
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    if state.closed.get() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    if msg == state.taskbar_created {
        state.taskbar_created();
        return LRESULT(0);
    }
    match msg {
        WM_TIMER if wp.0 == RETRY_TIMER => {
            if state.timer.get() {
                if let Err(error) = state.try_register() {
                    state.error(&error);
                }
            }
        }
        WM_TRAY => match tray_action(state.version4.get(), wp, lp) {
            Some(Action::Open) => state.invoke(OPEN),
            Some(Action::Menu(position)) => {
                if let Err(error) = state.popup_menu(position) {
                    state.error(&error);
                }
            }
            None => {}
        },
        WM_CONTEXTMENU => {
            if let Err(error) = state.popup_menu(Some(point_from_param(lp.0))) {
                state.error(&error);
            }
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}

fn ensure_window_class() -> Result<(), String> {
    static CLASS: OnceLock<Result<(), String>> = OnceLock::new();
    CLASS
        .get_or_init(|| {
            let instance = unsafe { GetModuleHandleW(None) }
                .map_err(|error| format!("GetModuleHandleW: {error}"))?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance.into(),
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            if unsafe { RegisterClassW(&class) } == 0 {
                Err(last_error("RegisterClassW"))
            } else {
                Ok(())
            }
        })
        .clone()
}

fn last_error(operation: &str) -> String {
    format!("{operation}: {}", windows::core::Error::from_thread())
}

fn diagnostic(event: &str, detail: &str) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        crate::log::write(
            &crate::paths::user_paths().state_dir,
            &serde_json::json!({
                "t": crate::log::mono_ms(),
                "ev": event,
                "pid": std::process::id(),
                "detail": detail,
            }),
        );
    }));
    if let Err(payload) = result {
        std::mem::forget(payload);
    }
}

struct Icon(HICON);
struct Menu(HMENU);
struct Bitmap(HBITMAP);

impl Drop for Icon {
    fn drop(&mut self) {
        let _ = unsafe { DestroyIcon(self.0) };
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        let _ = unsafe { DestroyMenu(self.0) };
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        let _ = unsafe { DeleteObject(self.0.into()) };
    }
}

fn create_menu() -> Result<Menu, String> {
    let menu = Menu(unsafe { CreatePopupMenu() }.map_err(|error| format!("CreatePopupMenu: {error}"))?);
    unsafe {
        AppendMenuW(menu.0, MF_STRING, OPEN as usize, w!("打开设置"))
            .map_err(|error| format!("AppendMenuW: {error}"))?;
        AppendMenuW(menu.0, MF_STRING, QUIT as usize, w!("退出"))
            .map_err(|error| format!("AppendMenuW: {error}"))?;
        SetMenuDefaultItem(menu.0, OPEN, 0)
            .map_err(|error| format!("SetMenuDefaultItem: {error}"))?;
    }
    Ok(menu)
}

fn create_icon() -> Result<Icon, String> {
    let image = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
        .map_err(|error| format!("托盘 PNG: {error}"))?
        .into_rgba8();
    let width = i32::try_from(image.width()).map_err(|error| error.to_string())?;
    let height = i32::try_from(image.height()).map_err(|error| error.to_string())?;
    if width == 0 || height == 0 {
        return Err("托盘图标尺寸为空".into());
    }
    let mut bgra = image.into_raw();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let color = Bitmap(
        unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }
            .map_err(|error| format!("CreateDIBSection: {error}"))?,
    );
    if bits.is_null() {
        return Err("CreateDIBSection 未返回像素存储".into());
    }
    unsafe { std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits.cast::<u8>(), bgra.len()) };
    let mask_bytes = vec![0u8; (width as usize).div_ceil(16) * 2 * height as usize];
    let mask = Bitmap(unsafe {
        CreateBitmap(width, height, 1, 1, Some(mask_bytes.as_ptr().cast()))
    });
    if mask.0.is_invalid() {
        return Err(last_error("CreateBitmap"));
    }
    let icon = unsafe {
        CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            hbmColor: color.0,
            hbmMask: mask.0,
            ..Default::default()
        })
    }
    .map_err(|error| format!("CreateIconIndirect: {error}"))?;
    Ok(Icon(icon))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use windows::Win32::UI::WindowsAndMessaging::{
        CopyIcon, GetAncestor, IsMenu, IsWindow, IsWindowVisible, SendMessageW, GA_ROOT,
        GWL_EXSTYLE,
    };

    #[derive(Default)]
    struct Shell {
        additions: RefCell<VecDeque<bool>>,
        calls: RefCell<Vec<u32>>,
        events: RefCell<Vec<String>>,
        version: Cell<u32>,
    }

    impl Shell {
        fn new(results: &[bool]) -> Rc<Self> {
            Rc::new(Self {
                additions: RefCell::new(results.iter().copied().collect()),
                ..Default::default()
            })
        }

        fn hooks(self: &Rc<Self>) -> (Notify, Report) {
            let shell = self.clone();
            let notify = Box::new(move |operation: NOTIFY_ICON_MESSAGE, data: &NOTIFYICONDATAW| {
                shell.calls.borrow_mut().push(operation.0);
                if operation == NIM_ADD {
                    shell.additions.borrow_mut().pop_front().unwrap_or(true)
                } else {
                    if operation == NIM_SETVERSION {
                        shell.version.set(unsafe { data.Anonymous.uVersion });
                    }
                    true
                }
            });
            let shell = self.clone();
            let report = Box::new(move |event: &str, _: &str| {
                shell.events.borrow_mut().push(event.into());
            });
            (notify, report)
        }

        fn tray(self: &Rc<Self>) -> Tray {
            let (notify, report) = self.hooks();
            Tray::with_hooks(|| {}, || {}, notify, report).unwrap()
        }

        fn count(&self, operation: NOTIFY_ICON_MESSAGE) -> usize {
            self.calls.borrow().iter().filter(|&&op| op == operation.0).count()
        }
    }

    fn send(hwnd: HWND, message: u32, wp: usize, lp: isize) {
        unsafe { SendMessageW(hwnd, message, Some(WPARAM(wp)), Some(LPARAM(lp))) };
    }

    fn tick(tray: &Tray) {
        send(tray.state.hwnd.get(), WM_TIMER, RETRY_TIMER, 0);
    }

    fn selection(event: u32) -> LPARAM {
        LPARAM(((ICON_ID << 16) | event) as isize)
    }

    #[test]
    fn retries_until_ready_without_log_flood() {
        let shell = Shell::new(&[false, false, true]);
        let tray = shell.tray();
        let hwnd = tray.state.hwnd.get();
        assert!(tray.state.timer.get());
        assert!(!tray.state.registered.get());
        assert_eq!(unsafe { GetAncestor(hwnd, GA_ROOT) }, hwnd);
        assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
        assert_ne!(unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW.0, 0);
        tick(&tray);
        assert_eq!(&*shell.events.borrow(), &["tray-wait"]);
        tick(&tray);
        assert!(tray.state.registered.get());
        assert!(!tray.state.timer.get());
        assert!(unsafe { KillTimer(Some(hwnd), RETRY_TIMER) }.is_err());
        assert_eq!(shell.version.get(), NOTIFYICON_VERSION_4);
        assert_eq!(&*shell.events.borrow(), &["tray-wait", "tray-ready"]);
        tick(&tray);
        assert_eq!(shell.count(NIM_ADD), 3);
        drop(tray);
        assert_eq!(shell.count(NIM_DELETE), 1);
    }

    #[test]
    fn taskbar_created_registers_again_and_retries() {
        let shell = Shell::new(&[true, false, true, true]);
        let tray = shell.tray();
        let hwnd = tray.state.hwnd.get();
        send(hwnd, tray.state.taskbar_created, 0, 0);
        assert!(!tray.state.registered.get());
        assert!(tray.state.timer.get());
        tick(&tray);
        assert!(tray.state.registered.get());
        assert!(!tray.state.timer.get());
        send(hwnd, tray.state.taskbar_created, 0, 0);
        assert!(tray.state.registered.get());
        assert_eq!(shell.count(NIM_ADD), 4);
        assert_eq!(shell.count(NIM_SETVERSION), 3);
        assert_eq!(&*shell.events.borrow(), &["tray-ready", "tray-wait", "tray-ready", "tray-ready"]);
    }

    #[test]
    fn drop_cancels_waiting_and_releases_native_resources() {
        for added in [false, true] {
            let shell = Shell::new(&[added]);
            let tray = shell.tray();
            let state = Rc::clone(&tray.state);
            let weak = Rc::downgrade(&state);
            let hwnd = state.hwnd.get();
            let menu = state.menu.0;
            let icon = state.icon.0;
            drop(tray);
            assert!(state.closed.get());
            assert!(!state.timer.get());
            assert!(!state.registered.get());
            assert!(state.hwnd.get().is_invalid());
            state.try_register().unwrap();
            state.taskbar_created();
            assert_eq!(shell.count(NIM_ADD), 1);
            assert_eq!(shell.count(NIM_DELETE), 1);
            assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
            drop(state);
            assert!(weak.upgrade().is_none());
            assert!(!unsafe { IsMenu(menu) }.as_bool());
            let copy = unsafe { CopyIcon(icon) }.map(Icon);
            assert!(copy.is_err());
        }
    }

    #[test]
    fn callback_can_drop_its_tray() {
        let owner = Rc::new(RefCell::new(None::<Tray>));
        let weak_owner = Rc::downgrade(&owner);
        let shell = Shell::new(&[true]);
        let (notify, report) = shell.hooks();
        let tray = Tray::with_hooks(
            move || {
                let owner = weak_owner.upgrade().unwrap();
                let tray = owner.borrow_mut().take();
                drop(tray);
            },
            || {},
            notify,
            report,
        )
        .unwrap();
        let hwnd = tray.state.hwnd.get();
        let weak_state = Rc::downgrade(&tray.state);
        *owner.borrow_mut() = Some(tray);
        send(hwnd, WM_TRAY, 0, selection(NIN_KEYSELECT).0);
        assert!(owner.borrow().is_none());
        assert!(weak_state.upgrade().is_none());
        assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
        assert_eq!(shell.count(NIM_DELETE), 1);
    }

    #[test]
    #[cfg(panic = "unwind")]
    fn callback_panic_and_reentry_do_not_escape_window_procedure() {
        let hwnd = Rc::new(Cell::new(HWND::default()));
        let calls = Rc::new(Cell::new(0));
        let callback_hwnd = hwnd.clone();
        let callback_calls = calls.clone();
        let shell = Shell::new(&[true]);
        let (notify, report) = shell.hooks();
        let tray = Tray::with_hooks(
            move || {
                let count = callback_calls.get();
                callback_calls.set(count + 1);
                if count == 0 {
                    send(callback_hwnd.get(), WM_TRAY, 0, selection(NIN_SELECT).0);
                    panic!("simulated callback panic");
                }
            },
            || {},
            notify,
            report,
        )
        .unwrap();
        hwnd.set(tray.state.hwnd.get());
        send(hwnd.get(), WM_TRAY, 0, selection(NIN_SELECT).0);
        assert_eq!(calls.get(), 1);
        assert_eq!(&*shell.events.borrow(), &["tray-ready", "tray-error"]);
        send(hwnd.get(), WM_TRAY, 0, selection(NIN_KEYSELECT).0);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn taskbar_broadcast_during_add_is_not_lost() {
        let shell = Shell::new(&[true, true]);
        let (notify, report) = shell.hooks();
        let first = Cell::new(true);
        let message = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        let tray = Tray::with_hooks(
            || {},
            || {},
            Box::new(move |operation, data| {
                let result = notify(operation, data);
                if operation == NIM_ADD && first.replace(false) {
                    send(data.hWnd, message, 0, 0);
                }
                result
            }),
            report,
        )
        .unwrap();
        assert_eq!(shell.count(NIM_ADD), 1);
        assert!(!tray.state.registered.get());
        assert!(tray.state.timer.get());
        tick(&tray);
        assert_eq!(shell.count(NIM_ADD), 2);
        assert!(tray.state.registered.get());
        assert!(!tray.state.timer.get());
    }

    #[test]
    fn native_keyboard_and_mouse_notifications_are_decoded_once() {
        for event in [NIN_SELECT, NIN_KEYSELECT] {
            assert_eq!(tray_action(true, WPARAM(0), selection(event)), Some(Action::Open));
        }
        assert_eq!(tray_action(true, WPARAM(0), selection(WM_LBUTTONUP)), None);
        let position = WPARAM(((40u32 << 16) | (-300i16 as u16 as u32)) as usize);
        assert_eq!(
            tray_action(true, position, selection(WM_CONTEXTMENU)),
            Some(Action::Menu(Some(POINT { x: -300, y: 40 }))),
        );
        assert_eq!(point_from_param(-1), POINT { x: -1, y: -1 });
        assert_eq!(tray_action(false, WPARAM(ICON_ID as usize), LPARAM(WM_LBUTTONUP as isize)), Some(Action::Open));
        assert_eq!(tray_action(false, WPARAM(ICON_ID as usize), LPARAM(WM_RBUTTONUP as isize)), Some(Action::Menu(None)));
        assert_eq!(tray_action(true, WPARAM(0), LPARAM(((2 << 16) | NIN_SELECT) as isize)), None);
    }

    #[test]
    fn embedded_icon_is_the_existing_64_pixel_png() {
        let image = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png).unwrap();
        assert_eq!((image.width(), image.height()), (64, 64));
    }
}
