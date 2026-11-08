//! The settings window, opened from the tray menu. A plain native window with standard
//! controls, created on demand and destroyed when closed, so it costs nothing otherwise.
//!
//! The checkboxes never hold state of their own: after every click they are set again from
//! the real settings, so they always agree with the tray menu and with Windows.

use crate::{store, ui};
use std::cell::Cell;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{LibraryLoader::GetModuleHandleW, Registry::*},
        UI::{
            HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow, SystemParametersInfoForDpi},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};

const CLASS: PCWSTR = w!("OrganizerSettings");
/// Resource id given to the app icon in build.rs.
const ICON_ID: u16 = 1;
const STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0);

const ID_OK: i32 = 1; // what Enter sends
const ID_CLOSE: i32 = 2; // IDCANCEL, what Esc sends
const ID_RECYCLE_BIN: i32 = 101;
const ID_LOCK: i32 = 102;
const ID_AUTOSTART: i32 = 103;
const ID_THEME: i32 = 104;
const ID_DOUBLE_CLICK: i32 = 105;

const BS_PUSHBUTTON: u32 = 0x0;
const BS_DEFPUSHBUTTON: u32 = 0x1;
const BS_AUTOCHECKBOX: u32 = 0x3;
const BM_SETCHECK: u32 = 0x00F1;

/// Where Windows keeps which of its own icons the desktop hides: the same switch as
/// Settings > Personalization > Themes > Desktop icon settings.
const HIDE_ICONS_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\HideDesktopIcons\\NewStartPanel");
const RECYCLE_BIN: PCWSTR = w!("{645FF040-5081-101B-9F08-00AA002F954E}");

/// Client size, in pixels at 100% scale.
const CLIENT: (i32, i32) = (400, 196);

struct Control {
    id: i32,
    text: PCWSTR,
    style: u32,
    /// x, y, width, height at 100% scale.
    rect: (i32, i32, i32, i32),
}

const CONTROLS: [Control; 6] = [
    Control { id: ID_RECYCLE_BIN, text: w!("Hide the Recycle Bin on the desktop"), style: BS_AUTOCHECKBOX, rect: (20, 18, 360, 22) },
    Control { id: ID_LOCK, text: w!("Lock boxes, so they can't be moved by accident"), style: BS_AUTOCHECKBOX, rect: (20, 48, 360, 22) },
    Control { id: ID_DOUBLE_CLICK, text: w!("Double-click the desktop to hide boxes and icons"), style: BS_AUTOCHECKBOX, rect: (20, 78, 360, 22) },
    Control { id: ID_AUTOSTART, text: w!("Start Organizer with Windows"), style: BS_AUTOCHECKBOX, rect: (20, 108, 360, 22) },
    Control { id: ID_THEME, text: w!("Customize design (theme.css)"), style: BS_PUSHBUTTON, rect: (20, 150, 220, 30) },
    Control { id: ID_CLOSE, text: w!("Close"), style: BS_DEFPUSHBUTTON, rect: (290, 150, 90, 30) },
];

thread_local! {
    static WINDOW: Cell<HWND> = Cell::new(HWND::default());
    static FONT: Cell<HFONT> = Cell::new(HFONT::default());
}

pub fn recycle_bin_hidden() -> bool {
    let (mut value, mut size) = (0u32, size_of::<u32>() as u32);
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            HIDE_ICONS_KEY,
            RECYCLE_BIN,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    read.is_ok() && value != 0
}

fn set_recycle_bin_hidden(hide: bool) {
    unsafe {
        let mut key = HKEY::default();
        let created = RegCreateKeyExW(HKEY_CURRENT_USER, HIDE_ICONS_KEY, None, None, REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None);
        if created.is_err() {
            store::log("could not open the desktop icon settings");
            return;
        }
        if RegSetValueExW(key, RECYCLE_BIN, None, REG_DWORD, Some(&(hide as u32).to_le_bytes())).is_err() {
            store::log("could not change the Recycle Bin setting");
        }
        let _ = RegCloseKey(key);
    }
    // Explorer only rereads the setting when the desktop refreshes.
    ui::refresh_desktop();
}

/// Opens the window, or brings it to the front if it is already open.
pub fn open() {
    let existing = WINDOW.get();
    unsafe {
        if !existing.is_invalid() {
            let _ = ShowWindow(existing, SW_RESTORE);
            let _ = SetForegroundWindow(existing);
            return;
        }
        let instance: HINSTANCE = GetModuleHandleW(None).expect("module handle").into();
        // Registering again later fails harmlessly, so there is no need to remember it.
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: CLASS,
            hIcon: LoadIconW(Some(instance), PCWSTR(ICON_ID as usize as *const u16)).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            // Checkboxes paint their background in this color, so the window matches it.
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            ..Default::default()
        });
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("Organizer settings"),
            STYLE,
            CW_USEDEFAULT, CW_USEDEFAULT, 0, 0,
            None, None, Some(instance), None,
        ) else {
            store::log("could not open the settings window");
            return;
        };
        WINDOW.set(hwnd);
        for c in &CONTROLS {
            let style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(c.style);
            let _ = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                c.text,
                style,
                0, 0, 0, 0,
                Some(hwnd), Some(HMENU(c.id as isize as *mut _)), Some(instance), None,
            );
        }
        layout(hwnd, GetDpiForWindow(hwnd), None);
        sync(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        if let Ok(first) = GetDlgItem(Some(hwnd), ID_RECYCLE_BIN) {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(first));
        }
    }
}

/// Updates the checkboxes if the window is open, after a change made elsewhere.
pub fn refresh() {
    let hwnd = WINDOW.get();
    if !hwnd.is_invalid() {
        sync(hwnd);
    }
}

/// Lets Tab, Enter and Esc work in the window, as they do in a dialog box. True if the
/// message was handled and must not be dispatched again.
pub fn translate(msg: &MSG) -> bool {
    let hwnd = WINDOW.get();
    !hwnd.is_invalid() && unsafe { IsDialogMessageW(hwnd, msg) }.as_bool()
}

fn sync(hwnd: HWND) {
    let set = |id: i32, on: bool| unsafe {
        if let Ok(c) = GetDlgItem(Some(hwnd), id) {
            SendMessageW(c, BM_SETCHECK, Some(WPARAM(on as usize)), None);
        }
    };
    set(ID_RECYCLE_BIN, recycle_bin_hidden());
    set(ID_LOCK, ui::locked());
    set(ID_DOUBLE_CLICK, ui::double_click_hides());
    set(ID_AUTOSTART, ui::autostart_enabled());
}

/// Sizes the window and its controls for a display scale, in the system's dialog font.
/// Without `suggested`, the window is centered on its monitor.
fn layout(hwnd: HWND, dpi: u32, suggested: Option<RECT>) {
    let dpi = dpi.max(96);
    let px = |v: i32| v * dpi as i32 / 96;
    unsafe {
        let mut metrics = NONCLIENTMETRICSW { cbSize: size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let font = match SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some(&mut metrics as *mut _ as *mut _),
            0,
            dpi,
        ) {
            Ok(()) => CreateFontIndirectW(&metrics.lfMessageFont),
            Err(_) => HFONT(GetStockObject(DEFAULT_GUI_FONT).0),
        };
        for c in &CONTROLS {
            let Ok(child) = GetDlgItem(Some(hwnd), c.id) else { continue };
            let (x, y, w, h) = c.rect;
            let _ = SetWindowPos(child, None, px(x), px(y), px(w), px(h), SWP_NOZORDER | SWP_NOACTIVATE);
            SendMessageW(child, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        }
        let old = FONT.replace(font);
        if !old.is_invalid() {
            let _ = DeleteObject(old.into());
        }

        let mut frame = RECT { left: 0, top: 0, right: px(CLIENT.0), bottom: px(CLIENT.1) };
        let _ = AdjustWindowRectExForDpi(&mut frame, STYLE, false, WINDOW_EX_STYLE(0), dpi);
        let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
        let (x, y) = match suggested {
            Some(r) => (r.left, r.top),
            None => {
                let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
                let _ = GetMonitorInfoW(monitor, &mut info);
                let work = info.rcWork;
                (work.left + (work.right - work.left - w) / 2, work.top + (work.bottom - work.top - h) / 2)
            }
        };
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            match (wp.0 & 0xFFFF) as i32 {
                ID_RECYCLE_BIN => set_recycle_bin_hidden(!recycle_bin_hidden()),
                ID_LOCK => ui::set_locked(!ui::locked()),
                ID_DOUBLE_CLICK => ui::set_double_click_hides(!ui::double_click_hides()),
                ID_AUTOSTART => ui::set_autostart(!ui::autostart_enabled()),
                ID_THEME => ui::open_theme(),
                ID_OK | ID_CLOSE => {
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                    return LRESULT(0);
                }
                _ => {}
            }
            sync(hwnd);
        }
        // Coming back to the window: a setting may have changed in the meantime.
        WM_ACTIVATE if (wp.0 & 0xFFFF) as u32 != WA_INACTIVE => sync(hwnd),
        WM_DPICHANGED => {
            let suggested = unsafe { *(lp.0 as *const RECT) };
            layout(hwnd, ((wp.0 >> 16) & 0xFFFF) as u32, Some(suggested));
        }
        WM_DESTROY => {
            WINDOW.set(HWND::default());
            let font = FONT.replace(HFONT::default());
            if !font.is_invalid() {
                unsafe {
                    let _ = DeleteObject(font.into());
                }
            }
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}
