//! Notices double-clicks and drops on the desktop. The desktop belongs to Explorer, so Organizer
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
    Graphics::Gdi::{RDW_ALLCHILDREN, RDW_ERASE, RDW_FRAME, RDW_INVALIDATE, RedrawWindow},
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetTickCount},
    UI::{
        Accessibility::{HWINEVENTHOOK, SetWinEventHook},
        Input::KeyboardAndMouse::{GetAsyncKeyState, GetDoubleClickTime, VK_LBUTTON},
        WindowsAndMessaging::*,
    },
};

static TARGET: AtomicIsize = AtomicIsize::new(0);
static MESSAGE: AtomicU32 = AtomicU32::new(0);
static RELEASE_MESSAGE: AtomicU32 = AtomicU32::new(0);
static DRAG_MESSAGE: AtomicU32 = AtomicU32::new(0);
/// Explorer's icon list, to freeze around drags, or 0 when nothing needs hiding.
static FREEZE_LIST: AtomicIsize = AtomicIsize::new(0);
/// The icon list currently has drawing switched off by `freeze`.
static FROZEN: AtomicIsize = AtomicIsize::new(0);
/// The longest the icons may stay frozen, whatever happens on the UI thread.
const FREEZE_LIMIT_MS: u32 = 500;
/// Most a cross-process message from the hook may hold up the mouse.
const FREEZE_SEND_MS: u32 = 30;

thread_local! {
    static HOOK: Cell<Option<HHOOK>> = const { Cell::new(None) };
    /// Time and place of the last left-button press, while it could start a double-click.
    static LAST: Cell<Option<(u32, POINT)>> = const { Cell::new(None) };
    /// Where the left button went down, while it is held, and whether it has become a drag.
    static PRESS: Cell<Option<(POINT, bool)>> = const { Cell::new(None) };
    /// The timer that lifts a freeze no matter what, restarted by each freeze.
    static SAFETY: Cell<usize> = const { Cell::new(0) };
}

/// Explorer pulls the icons of rolled-up boxes, which it sees as off the desktop, back
/// onto it when a drag starts or ends. To keep that from ever being seen, drawing of the
/// icons is switched off at those moments, before Explorer gets the mouse event, until
/// Organizer has parked them again (`unfreeze`). `list` is the icon list, or `None` when no
/// box is rolled up.
pub fn set_freeze_target(list: Option<HWND>) {
    FREEZE_LIST.store(list.map_or(0, |h| h.0 as isize), Ordering::Relaxed);
}

pub fn is_frozen() -> bool {
    FROZEN.load(Ordering::Relaxed) != 0
}

/// Runs on the hook thread, so it takes effect before Explorer sees the mouse event that
/// caused it: a sent message is handled before Explorer can read input queued after it.
fn freeze() {
    let list = FREEZE_LIST.load(Ordering::Relaxed);
    if list == 0 || FROZEN.compare_exchange(0, list, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return;
    }
    unsafe {
        let hwnd = HWND(list as *mut _);
        let _ = SendMessageTimeoutW(hwnd, WM_SETREDRAW, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, FREEZE_SEND_MS, None);
        // Never left frozen: this thread lifts it on its own if the UI thread does not.
        let old = SAFETY.with(|s| s.take());
        if old != 0 {
            let _ = KillTimer(None, old);
        }
        SAFETY.with(|s| s.set(SetTimer(None, 0, FREEZE_LIMIT_MS, Some(safety_unfreeze))));
    }
}

unsafe extern "system" fn safety_unfreeze(_: HWND, _: u32, id: usize, _: u32) {
    unsafe {
        let _ = KillTimer(None, id);
    }
    SAFETY.with(|s| s.set(0));
    unfreeze();
}

/// Switches drawing back on and redraws the icons once, as they are now. Safe from any
/// thread and when not frozen.
pub fn unfreeze() {
    let list = FROZEN.swap(0, Ordering::Relaxed);
    if list == 0 {
        return;
    }
    unsafe {
        let hwnd = HWND(list as *mut _);
        let _ = SendMessageTimeoutW(hwnd, WM_SETREDRAW, WPARAM(1), LPARAM(0), SMTO_ABORTIFHUNG, 200, None);
        let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN);
    }
}

/// True for the desktop's top-level windows.
pub fn is_desktop(hwnd: HWND) -> bool {
    let mut class = [0u16; 16];
    let n = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
    matches!(String::from_utf16_lossy(&class[..n]).as_str(), "Progman" | "WorkerW")
}

/// Starts watching. Each double-click while the desktop is in the foreground posts `msg`
/// to `notify`, with the screen point packed in lparam (x low word, y high word). Each
/// release of the left button there posts `release_msg` (wparam 1 if it ended a drag), and
/// the start of each drag posts `drag_msg`.
pub fn start(notify: HWND, msg: u32, release_msg: u32, drag_msg: u32) {
    TARGET.store(notify.0 as isize, Ordering::Relaxed);
    MESSAGE.store(msg, Ordering::Relaxed);
    RELEASE_MESSAGE.store(release_msg, Ordering::Relaxed);
    DRAG_MESSAGE.store(drag_msg, Ordering::Relaxed);
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
                PRESS.with(|p| p.set(None));
            }
            (false, Some(h)) => {
                let _ = UnhookWindowsHookEx(h);
                hook.set(None);
                PRESS.with(|p| p.set(None));
            }
            _ => {}
        }
    });
}

/// Must return quickly: it holds up every mouse event while the desktop is in front.
unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let post = |msg: &AtomicU32, wparam: usize| unsafe {
        let target = HWND(TARGET.load(Ordering::Relaxed) as *mut _);
        let _ = PostMessageW(Some(target), msg.load(Ordering::Relaxed), WPARAM(wparam), LPARAM(0));
    };
    if code == HC_ACTION as i32 {
        let pt = unsafe { (*(lp.0 as *const MSLLHOOKSTRUCT)).pt };
        match wp.0 as u32 {
            WM_LBUTTONDOWN => PRESS.with(|p| p.set(Some((pt, false)))),
            WM_MOUSEMOVE => {
                // A press seen before the hook went away and came back is long over.
                let held = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 != 0;
                if let Some((from, false)) = PRESS.with(|p| p.get()).filter(|_| held) {
                    let far = unsafe {
                        (pt.x - from.x).abs() > GetSystemMetrics(SM_CXDRAG)
                            || (pt.y - from.y).abs() > GetSystemMetrics(SM_CYDRAG)
                    };
                    if far {
                        PRESS.with(|p| p.set(Some((from, true))));
                        freeze();
                        post(&DRAG_MESSAGE, 0);
                    }
                }
            }
            WM_LBUTTONUP => {
                let dragged = PRESS.with(|p| p.take()).is_some_and(|(_, dragged)| dragged);
                if dragged {
                    freeze();
                }
                post(&RELEASE_MESSAGE, dragged as usize);
            }
            _ => {}
        }
    }
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
