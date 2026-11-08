//! Notices double-clicks on the desktop. The desktop belongs to Explorer, so Organizer
//! never receives its clicks; a low-level mouse hook sees them instead.
//!
//! A mouse hook makes every mouse event in the system pass through this process, so it
//! exists only while the desktop is the foreground window. Windows reports foreground
//! changes as events (nothing is polled), and in a game or any other app no hook is
//! installed at all. The hook runs on its own thread, which does nothing else, so the
//! mouse never waits for the UI thread or for Explorer.

use std::{
    cell::Cell,
    sync::atomic::{AtomicIsize, AtomicU32, Ordering},
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetTickCount},
    UI::{
        Accessibility::{HWINEVENTHOOK, SetWinEventHook},
        Input::KeyboardAndMouse::{GetAsyncKeyState, GetDoubleClickTime, VK_LBUTTON},
        WindowsAndMessaging::*,
    },
};

static TARGET: AtomicIsize = AtomicIsize::new(0);
static MESSAGE: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static HOOK: Cell<Option<HHOOK>> = const { Cell::new(None) };
    /// Time and place of the last left-button press, while it could start a double-click.
    static LAST: Cell<Option<(u32, POINT)>> = const { Cell::new(None) };
}

/// True for the desktop's top-level windows.
pub fn is_desktop(hwnd: HWND) -> bool {
    let mut class = [0u16; 16];
    let n = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
    matches!(String::from_utf16_lossy(&class[..n]).as_str(), "Progman" | "WorkerW")
}

/// Starts watching. Each double-click while the desktop is in the foreground posts `msg`
/// to `notify`, with the screen point packed in lparam (x low word, y high word).
pub fn start(notify: HWND, msg: u32) {
    TARGET.store(notify.0 as isize, Ordering::Relaxed);
    MESSAGE.store(msg, Ordering::Relaxed);
    let started = std::thread::Builder::new().name("desktop-clicks".into()).spawn(|| unsafe {
        // Out-of-context events arrive as messages to this thread, in the loop below.
        SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT);
        update_hook(GetForegroundWindow(), false);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
    });
    if let Err(e) = started {
        crate::store::log(&format!("cannot watch for desktop double-clicks: {e}"));
    }
}

unsafe extern "system" fn on_foreground(_: HWINEVENTHOOK, _: u32, hwnd: HWND, _: i32, _: i32, _: u32, _: u32) {
    update_hook(hwnd, true);
}

/// Installs the hook while `foreground` is the desktop, and removes it otherwise.
fn update_hook(foreground: HWND, from_event: bool) {
    let want = is_desktop(foreground);
    HOOK.with(|hook| unsafe {
        match (want, hook.get()) {
            (true, None) => {
                let module = GetModuleHandleW(None).ok().map(Into::into);
                let Ok(h) = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0) else { return };
                hook.set(Some(h));
                // A click that brought the desktop forward came before the hook existed.
                // Count it, so the very first double-click on the desktop works too.
                let pressed = GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000 != 0;
                let mut cursor = POINT::default();
                let first = (from_event && pressed && GetCursorPos(&mut cursor).is_ok()).then(|| (GetTickCount(), cursor));
                LAST.with(|l| l.set(first));
            }
            (false, Some(h)) => {
                let _ = UnhookWindowsHookEx(h);
                hook.set(None);
            }
            _ => {}
        }
    });
}

/// Must return quickly: it holds up every mouse event while the desktop is in front.
unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && wp.0 as u32 == WM_LBUTTONDOWN {
        let info = unsafe { &*(lp.0 as *const MSLLHOOKSTRUCT) };
        let (time, pt) = (info.time, info.pt);
        let double = LAST.with(|l| l.get()).is_some_and(|(t, p)| unsafe {
            time.wrapping_sub(t) <= GetDoubleClickTime()
                && (pt.x - p.x).abs() <= GetSystemMetrics(SM_CXDOUBLECLK) / 2
                && (pt.y - p.y).abs() <= GetSystemMetrics(SM_CYDOUBLECLK) / 2
        });
        // After a double-click, the next press starts over, so a triple-click toggles once.
        LAST.with(|l| l.set((!double).then_some((time, pt))));
        if double {
            let packed = (pt.y as u16 as u32) << 16 | pt.x as u16 as u32;
            let target = HWND(TARGET.load(Ordering::Relaxed) as *mut _);
            unsafe {
                let _ = PostMessageW(Some(target), MESSAGE.load(Ordering::Relaxed), WPARAM(0), LPARAM(packed as isize));
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}
