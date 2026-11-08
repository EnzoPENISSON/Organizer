//! Tray icon, box windows and the polling loop.
//!
//! Each box is two child windows of Explorer's desktop view, which keeps it on the
//! desktop, out of Alt-Tab, and visible through Win+D:
//! - a tint: the body and glow, always click-through so the real icons stay usable;
//! - a frame: title bar and border. Its fully transparent middle lets clicks through.
//! Both are drawn by `render` only when their look or size changes.
//!
//! Re-entrancy rule: COM calls into Explorer and most window calls can dispatch messages
//! back into these window procedures, so `APP` is never borrowed across one. `with_app`
//! closures only read or change data.

use crate::{
    desktop::Desktop,
    model::{self, DesktopBox, Edges, Metrics, Rect},
    render::{self, Layer, Sizes},
    store::{self, Snapshot, State},
    theme::{Align, Theme},
    update, watch,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::SystemTime};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{
            LibraryLoader::GetModuleHandleW,
            Registry::*,
            Threading::{
                CreateMutexW, GetCurrentProcess, OpenProcess, PROCESS_SYNCHRONIZE, SetProcessWorkingSetSize,
                WaitForSingleObject,
            },
        },
        UI::{
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, GetLastInputInfo, LASTINPUTINFO, ReleaseCapture, SetCapture, SetFocus,
                VK_ESCAPE, VK_LBUTTON, VK_RETURN,
            },
            Shell::{
                NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
                NIN_BALLOONUSERCLICK, NOTIFYICONDATAW, QUNS_BUSY,
                QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
                ShellExecuteW, Shell_NotifyIconW,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCWSTR, w},
};

const FRAME_CLASS: PCWSTR = w!("OrganizerBoxFrame");
const TINT_CLASS: PCWSTR = w!("OrganizerBoxTint");
const HOST_CLASS: PCWSTR = w!("OrganizerHost");
const WM_TRAY: u32 = WM_APP + 1;
const WM_FOLLOW: u32 = WM_APP + 2;
/// Posted by `organizer.exe new` to the running instance.
const WM_NEW_BOX: u32 = WM_APP + 3;
/// Posted by the update check's thread when it is done. See `update::check_async`.
const WM_UPDATE: u32 = WM_APP + 4;
/// Posted by `organizer.exe exit` (the installer and uninstaller).
const WM_EXIT: u32 = WM_APP + 5;
/// Posted when the update installer has been downloaded. See `update::download_async`.
const WM_UPDATE_DOWNLOADED: u32 = WM_APP + 6;
/// Posted by `watch` for a double-click while the desktop is in front.
const WM_DESKTOP_DOUBLE_CLICK: u32 = WM_APP + 7;
const FADE_TIMER: usize = 3;
const FADE_MS: f32 = 200.0;
/// About 60 frames a second, only while a fade runs.
const FADE_FRAME_MS: u32 = 16;
const UPDATE_TIMER: usize = 2;
/// The first check waits a minute so it never competes with startup work.
const UPDATE_FIRST_MS: u32 = 60 * 1000;
const UPDATE_EVERY_MS: u32 = 12 * 60 * 60 * 1000;
const EM_SETSEL: u32 = 0x00B1;
const POLL_TIMER: usize = 1;
const POLL_MS: u32 = 1000;
const TICKS_PER_TRIM: u32 = 30;
/// Catches what happens away from the desktop, like a file deleted from another window.
const TICKS_PER_FULL_CHECK: u32 = 30;
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Organizer");
/// The desktop background's right-click menu. On Windows 11 it is under "Show more options".
const MENU_KEY: PCWSTR = w!("Software\\Classes\\DesktopBackground\\Shell\\OrganizerNewBox");
const MENU_COMMAND_KEY: PCWSTR = w!("Software\\Classes\\DesktopBackground\\Shell\\OrganizerNewBox\\command");

const CMD_NEW: usize = 1;
const CMD_LOCK: usize = 2;
const CMD_AUTOSTART: usize = 3;
const CMD_RESTORE: usize = 4;
const CMD_EXIT: usize = 5;
const CMD_THEME: usize = 6;
const CMD_CHECK_UPDATE: usize = 7;
const CMD_INSTALL_UPDATE: usize = 8;
const CMD_HIDE: usize = 9;
const CMD_RENAME: usize = 10;
const CMD_DELETE: usize = 11;

struct Drag {
    id: u64,
    edges: Edges,
    start: Rect,
    cursor: POINT,
}

struct Rename {
    id: u64,
    edit: HWND,
    old_proc: isize,
}

#[derive(Default)]
struct App {
    state: State,
    host: HWND,
    tray_icon: HICON,
    desktop: Option<Desktop>,
    metrics: Option<Metrics>,
    windows: HashMap<u64, (HWND, HWND)>,
    drag: Option<Drag>,
    rename: Option<Rename>,
    polling: bool,
    ticks_since_trim: u32,
    worked_since_trim: bool,
    ticks_since_check: u32,
    last_input: u32,
    desktop_was_active: bool,
    theme: Rc<Theme>,
    theme_stamp: Option<SystemTime>,
    rendered_size: HashMap<u64, (i32, i32)>,
    follow_pending: bool,
    /// A newer release, once a check has found one.
    update: Option<update::Release>,
    checking_update: bool,
    downloading_update: bool,
    fade: Fade,
}

/// Hiding the desktop: boxes fade, icons switch off. Not saved, so Organizer always starts
/// with everything shown.
struct Fade {
    hidden: bool,
    /// Current opacity of every box window.
    alpha: u8,
    /// Opacity when the running fade started, and when it started.
    from: u8,
    start: Option<std::time::Instant>,
}

impl Default for Fade {
    fn default() -> Self {
        Self { hidden: false, alpha: 255, from: 255, start: None }
    }
}

thread_local! {
    static APP: RefCell<App> = RefCell::default();
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> R {
    APP.with(|a| f(&mut a.borrow_mut()))
}

fn loword(v: isize) -> i32 {
    (v & 0xFFFF) as i16 as i32
}
fn hiword(v: isize) -> i32 {
    ((v >> 16) & 0xFFFF) as i16 as i32
}
fn box_id(hwnd: HWND) -> u64 {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as u64 }
}

fn find_box(id: u64) -> Option<DesktopBox> {
    with_app(|a| a.state.boxes.iter().find(|b| b.id == id).cloned())
}

fn save() {
    with_app(|a| store::save(&a.state));
}

/// Asks a running Organizer to quit and waits up to 10 seconds for it to finish, so its
/// files can be replaced. Used by the installer and uninstaller: `organizer.exe exit`.
pub fn request_exit() {
    unsafe {
        let Ok(host) = FindWindowW(HOST_CLASS, None) else { return };
        let mut pid = 0;
        GetWindowThreadProcessId(host, Some(&mut pid));
        let process = OpenProcess(PROCESS_SYNCHRONIZE, false, pid);
        if PostMessageW(Some(host), WM_EXIT, WPARAM(0), LPARAM(0)).is_ok() {
            if let Ok(process) = process {
                WaitForSingleObject(process, 10_000);
                let _ = CloseHandle(process);
            }
        }
    }
}

/// Asks an already running Organizer to create a box. False if none is running.
pub fn request_new_box() -> bool {
    unsafe {
        match FindWindowW(HOST_CLASS, None) {
            Ok(host) if !host.is_invalid() => PostMessageW(Some(host), WM_NEW_BOX, WPARAM(0), LPARAM(0)).is_ok(),
            _ => false,
        }
    }
}

pub fn run(start_with_new_box: bool) {
    unsafe {
        // Two instances would fight over the same icons.
        let mutex = CreateMutexW(None, false, w!("Local\\OrganizerDesktopBoxes.v2"));
        if GetLastError() == ERROR_ALREADY_EXISTS || mutex.is_err() {
            return;
        }

        let instance: HINSTANCE = GetModuleHandleW(None).expect("module handle").into();
        let classes: [(PCWSTR, WNDPROC, WNDCLASS_STYLES); 3] = [
            (FRAME_CLASS, Some(frame_proc), CS_DBLCLKS),
            (TINT_CLASS, Some(tint_proc), WNDCLASS_STYLES(0)),
            (HOST_CLASS, Some(host_proc), WNDCLASS_STYLES(0)),
        ];
        for (name, proc, style) in classes {
            RegisterClassW(&WNDCLASSW {
                style,
                lpfnWndProc: proc,
                hInstance: instance,
                lpszClassName: name,
                ..Default::default()
            });
        }

        let host = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            HOST_CLASS,
            w!("Organizer"),
            WS_POPUP,
            0, 0, 0, 0,
            None, None, Some(instance), None,
        )
        .expect("host window");

        let icon = make_tray_icon();
        with_app(|a| {
            a.state = store::load();
            a.theme = Rc::new(store::load_theme());
            a.theme_stamp = store::theme_stamp();
            a.host = host;
            a.tray_icon = icon;
        });
        add_tray_icon();
        register_desktop_menu();
        connect();
        if start_with_new_box {
            new_box();
        }
        SetTimer(Some(host), POLL_TIMER, POLL_MS, None);
        if update::is_release_build() {
            SetTimer(Some(host), UPDATE_TIMER, UPDATE_FIRST_MS, None);
        }
        watch::start(host, WM_DESKTOP_DOUBLE_CLICK);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Never leave the desktop without icons.
        if let Some(d) = with_app(|a| a.desktop.clone().filter(|_| a.fade.hidden)) {
            let _ = d.set_icons_visible(true);
        }
        remove_tray_icon();
        unregister_desktop_menu();
        destroy_box_windows();
        save();
        let _ = DestroyWindow(host);
        // Release Explorer's COM objects now, while COM still runs. Left in the thread-local
        // `APP`, they would be released during process teardown and Explorer would keep this
        // process alive, as an unkillable zombie, until Explorer itself restarts.
        let app = with_app(std::mem::take);
        drop(app);
        drop(mutex);
    }
}

// ---- Tray ----

/// The app icon embedded by build.rs (from assets/organizer.ico), at the tray's size.
unsafe fn make_tray_icon() -> HICON {
    /// Resource id given to the icon in build.rs.
    const ICON_ID: u16 = 1;
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).expect("module handle").into();
        let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        LoadImageW(Some(instance), PCWSTR(ICON_ID as usize as *const u16), IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR)
            .map(|h| HICON(h.0))
            .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default())
    }
}

fn tray_data() -> NOTIFYICONDATAW {
    let (host, icon) = with_app(|a| (a.host, a.tray_icon));
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: host,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    fill_wstr(&mut data.szTip, "Organizer - desktop boxes");
    data
}

/// Copies `text` into a fixed, zeroed UTF-16 buffer, truncating to leave its null.
fn fill_wstr(buf: &mut [u16], text: &str) {
    let room = buf.len() - 1;
    for (slot, c) in buf[..room].iter_mut().zip(text.encode_utf16()) {
        *slot = c;
    }
}

/// A notification from the tray icon. Clicking it sends `NIN_BALLOONUSERCLICK`.
fn notify(title: &str, text: &str) {
    let mut data = tray_data();
    data.uFlags |= NIF_INFO;
    data.dwInfoFlags = NIIF_INFO;
    fill_wstr(&mut data.szInfoTitle, title);
    fill_wstr(&mut data.szInfo, text);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

// ---- Updates ----

fn start_update_check(manual: bool) {
    let (host, busy) = with_app(|a| (a.host, std::mem::replace(&mut a.checking_update, true)));
    if !busy && !update::check_async(host, WM_UPDATE, manual) {
        with_app(|a| a.checking_update = false);
    }
}

/// Automatic checks speak up only the first time they see a given release; a check from
/// the menu always answers.
fn on_update_checked(manual: bool, outcome: update::Outcome) {
    with_app(|a| a.checking_update = false);
    match outcome {
        update::Outcome::Newer(release) => {
            let seen = with_app(|a| a.update.as_ref().is_some_and(|r| r.tag == release.tag));
            if manual || !seen {
                let action = if release.installer.is_empty() { "open the download page" } else { "install it" };
                notify("Organizer update available", &format!("{} is available. Click to {action}.", release.tag));
            }
            with_app(|a| a.update = Some(release));
        }
        update::Outcome::UpToDate => {
            if manual {
                notify("Organizer is up to date", &format!("You have the latest version: {}.", update::label()));
            }
        }
        update::Outcome::Failed(e) => {
            store::log(&format!("update check failed: {e}"));
            if manual {
                notify("Could not check for updates", &e);
            }
        }
    }
}

/// Downloads the new release's installer, or opens its page if it has none.
fn install_update() {
    let Some(release) = with_app(|a| a.update.clone()) else { return };
    if with_app(|a| a.downloading_update) {
        return;
    }
    let host = with_app(|a| a.host);
    if update::download_async(host, WM_UPDATE_DOWNLOADED, &release) {
        with_app(|a| a.downloading_update = true);
        notify("Downloading Organizer update", &format!("{} will install in a moment.", release.tag));
    } else {
        open_update_page(&release);
    }
}

fn open_update_page(release: &update::Release) {
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(release.page.as_str()), None, None, SW_SHOWNORMAL);
    }
}

/// Runs the downloaded installer and quits so it can replace the exe. `/SILENT` shows its
/// progress but asks nothing, and it restarts Organizer when done (see installer/organizer.iss).
fn on_update_downloaded(result: Result<std::path::PathBuf, String>) {
    with_app(|a| a.downloading_update = false);
    let started = result.and_then(|path| unsafe {
        let args = w!("/SILENT /SUPPRESSMSGBOXES /NORESTART");
        let run = ShellExecuteW(None, w!("open"), &HSTRING::from(path.as_os_str()), args, None, SW_SHOWNORMAL);
        // Values of 32 or less are errors.
        if run.0 as isize > 32 { Ok(()) } else { Err(format!("could not start the installer (error {})", run.0 as isize)) }
    });
    match started {
        Ok(()) => unsafe { PostQuitMessage(0) },
        Err(e) => {
            store::log(&format!("update failed: {e}"));
            notify("Could not install the update", &e);
        }
    }
}

fn add_tray_icon() {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &tray_data());
    }
}

fn remove_tray_icon() {
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data());
    }
}

fn autostart_enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, None).is_ok() }
}

fn set_autostart(enable: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key).is_err() {
            store::log("could not open the Run key");
            return;
        }
        if enable {
            let _ = RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(&reg_sz(&exe_command(""))));
        } else {
            let _ = RegDeleteValueW(key, RUN_VALUE);
        }
        let _ = RegCloseKey(key);
    }
}

/// This exe, quoted, followed by `args`.
fn exe_command(args: &str) -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    format!("\"{}\"{}{}", exe.display(), if args.is_empty() { "" } else { " " }, args)
}

/// A REG_SZ value: UTF-16 with a terminating null.
fn reg_sz(text: &str) -> Vec<u8> {
    text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

fn set_default_value(subkey: PCWSTR, text: &str) -> bool {
    unsafe {
        let mut key = HKEY::default();
        let created = RegCreateKeyExW(HKEY_CURRENT_USER, subkey, None, None, REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None);
        if created.is_err() {
            return false;
        }
        let ok = RegSetValueExW(key, None, None, REG_SZ, Some(&reg_sz(text))).is_ok();
        let _ = RegCloseKey(key);
        ok
    }
}

/// Adds "New box" to the desktop's right-click menu. Rewritten at every start, so the
/// command follows the exe if it moves.
fn register_desktop_menu() {
    if !(set_default_value(MENU_KEY, "New box") && set_default_value(MENU_COMMAND_KEY, &exe_command("new"))) {
        store::log("could not add New box to the desktop menu");
    }
}

fn unregister_desktop_menu() {
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, MENU_KEY);
    }
}

/// Runs a popup menu and returns the chosen command. The menu needs its owner in the
/// foreground to close when the user clicks elsewhere.
unsafe fn track_menu(menu: HMENU) -> usize {
    unsafe {
        let host = with_app(|a| a.host);
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let _ = SetForegroundWindow(host);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, Some(0), host, None);
        let _ = PostMessageW(Some(host), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    }
}

fn show_tray_menu() {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let locked = with_app(|a| a.state.locked);
        let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING, CMD_NEW, w!("New box"));
        let _ = AppendMenuW(menu, MF_STRING | check(locked), CMD_LOCK, w!("Lock boxes"));
        let hidden = with_app(|a| a.fade.hidden);
        let _ = AppendMenuW(menu, MF_STRING | check(hidden), CMD_HIDE, w!("Hide boxes and icons\tDouble-click desktop"));
        let _ = AppendMenuW(menu, MF_STRING, CMD_THEME, w!("Customize design (theme.css)"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING | check(autostart_enabled()), CMD_AUTOSTART, w!("Start with Windows"));
        let _ = AppendMenuW(menu, MF_STRING, CMD_RESTORE, w!("Remove all boxes and restore original layout..."));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, &HSTRING::from(update::label()));
        if let Some(release) = with_app(|a| a.update.clone()) {
            let downloading = with_app(|a| a.downloading_update);
            let item = HSTRING::from(if downloading {
                format!("Downloading update {}...", release.tag)
            } else if !release.installer.is_empty() {
                format!("Install update {}", release.tag)
            } else {
                format!("Download update {}", release.tag)
            });
            let flags = MF_STRING | if downloading { MF_GRAYED } else { MF_ENABLED };
            let _ = AppendMenuW(menu, flags, CMD_INSTALL_UPDATE, &item);
        }
        let checking = with_app(|a| a.checking_update);
        let label = if checking { w!("Checking for updates...") } else { w!("Check for updates") };
        let _ = AppendMenuW(menu, MF_STRING | if checking { MF_GRAYED } else { MF_ENABLED }, CMD_CHECK_UPDATE, label);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, CMD_EXIT, w!("Exit"));
        match track_menu(menu) {
            CMD_NEW => new_box(),
            CMD_LOCK => {
                with_app(|a| a.state.locked = !a.state.locked);
                save();
            }
            CMD_THEME => open_theme(),
            CMD_HIDE => set_hidden(!hidden),
            CMD_AUTOSTART => set_autostart(!autostart_enabled()),
            CMD_RESTORE => restore_original(),
            CMD_CHECK_UPDATE => start_update_check(true),
            CMD_INSTALL_UPDATE => install_update(),
            CMD_EXIT => PostQuitMessage(0),
            _ => {}
        }
    }
}

fn open_theme() {
    if !store::theme_path().exists() {
        store::load_theme();
    }
    let path = HSTRING::from(store::theme_path().as_os_str());
    unsafe {
        let opened = ShellExecuteW(None, w!("open"), &path, None, None, SW_SHOWNORMAL);
        // Values of 32 or less are errors, such as no app being registered for .css files.
        if opened.0 as isize <= 32 {
            ShellExecuteW(None, w!("open"), w!("notepad.exe"), &path, None, SW_SHOWNORMAL);
        }
    }
}

fn show_box_menu(id: u64) {
    unsafe {
        if find_box(id).is_none() {
            return;
        }
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, CMD_NEW, w!("New box"));
        let _ = AppendMenuW(menu, MF_STRING, CMD_RENAME, w!("Rename"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, CMD_DELETE, w!("Delete box (icons stay on the desktop)"));
        match track_menu(menu) {
            CMD_NEW => new_box(),
            CMD_RENAME => begin_rename(id),
            CMD_DELETE => delete_box(id),
            _ => {}
        }
    }
}

// ---- Desktop connection ----

/// (Re)builds everything tied to Explorer. Run at startup and whenever Explorer
/// restarts, which destroys all our child windows along with their parent.
fn connect() -> bool {
    let desktop = match Desktop::connect() {
        Ok(d) => d,
        Err(e) => {
            store::log(&format!("cannot reach the desktop yet: {e}"));
            with_app(|a| a.desktop = None);
            return false;
        }
    };
    let metrics = read_metrics(&desktop).ok();
    // Explorer restarted while the desktop was hidden: start over with everything shown.
    if with_app(|a| std::mem::take(&mut a.fade).hidden) {
        let _ = desktop.set_icons_visible(true);
    }
    destroy_box_windows();
    with_app(|a| {
        a.desktop = Some(desktop);
        a.metrics = metrics;
    });
    for id in box_ids() {
        create_box_windows(id);
    }
    place_all();
    trim_memory();
    true
}

fn read_metrics(d: &Desktop) -> windows::core::Result<Metrics> {
    let t = with_app(|a| a.theme.clone());
    d.metrics(t.title_height, t.border_width, t.padding)
}

/// Hands pages back to Windows. Organizer idles nearly all the time, and most of its
/// working set is shell DLL code it only needed at startup or to show a menu.
fn trim_memory() {
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// Work area of the monitor a rectangle is on, in view coordinates.
fn area_of(view: HWND, r: &Rect) -> Rect {
    unsafe {
        let mut screen = [POINT { x: r.x, y: r.y }, POINT { x: r.x + r.w, y: r.y + r.h }];
        MapWindowPoints(Some(view), None, &mut screen);
        let rc = RECT { left: screen[0].x, top: screen[0].y, right: screen[1].x, bottom: screen[1].y };
        let monitor = MonitorFromRect(&rc, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let mut pts = [
            POINT { x: info.rcWork.left, y: info.rcWork.top },
            POINT { x: info.rcWork.right, y: info.rcWork.bottom },
        ];
        MapWindowPoints(None, Some(view), &mut pts);
        Rect { x: pts[0].x, y: pts[0].y, w: pts[1].x - pts[0].x, h: pts[1].y - pts[0].y }
    }
}

/// Saves the layout once, before Organizer first changes anything, and then frees icon
/// placement. Does nothing while there are no boxes, so a box-less Organizer leaves the
/// desktop's settings alone.
fn prepare_desktop(d: &Desktop) -> windows::core::Result<()> {
    if with_app(|a| a.state.boxes.is_empty()) {
        return Ok(());
    }
    if !store::has_snapshot() {
        let snapshot = Snapshot {
            folder_flags: d.folder_flags()?,
            icons: d.icons()?.into_iter().map(|i| (i.key, i.x, i.y)).collect(),
        };
        if let Err(e) = store::save_snapshot(&snapshot) {
            store::log(&format!("could not save the original layout: {e}"));
        }
    }
    d.allow_free_placement()
}

/// Moves every box's icons to their slots and syncs windows to the settled rectangles.
fn place_all() {
    let Some((d, m)) = with_app(|a| a.desktop.clone().zip(a.metrics)) else { return };
    if let Err(e) = prepare_desktop(&d) {
        store::log(&format!("could not prepare the desktop: {e}"));
        return;
    }
    let view = d.hwnd();
    let (moves, ids) = with_app(|a| {
        let moves = model::place_members(&mut a.state.boxes, &m, &|r| area_of(view, r));
        (moves, a.state.boxes.iter().map(|b| b.id).collect::<Vec<_>>())
    });
    for id in ids {
        sync_windows(id);
    }
    if let Err(e) = d.apply(&moves) {
        store::log(&format!("could not move icons: {e}"));
    }
    save();
}

fn poll() {
    // Stay out of the way mid-gesture: a half-finished drag is not a final position.
    let busy = with_app(|a| a.polling || a.drag.is_some() || a.rename.is_some());
    if busy || game_is_running() || unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 != 0 {
        return;
    }
    with_app(|a| a.polling = true);
    poll_inner();
    let trim = with_app(|a| {
        a.polling = false;
        a.ticks_since_trim += 1;
        let due = a.ticks_since_trim >= TICKS_PER_TRIM && a.worked_since_trim;
        if due {
            a.ticks_since_trim = 0;
            a.worked_since_trim = false;
        }
        due
    });
    if trim {
        trim_memory();
    }
}

/// True while a fullscreen app has the screen. Nobody can see or move the boxes then, so
/// Organizer stops touching Explorer and costs a game nothing.
fn game_is_running() -> bool {
    matches!(
        unsafe { SHQueryUserNotificationState() },
        Ok(QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_BUSY | QUNS_PRESENTATION_MODE)
    )
}

fn desktop_is_active() -> bool {
    watch::is_desktop(unsafe { GetForegroundWindow() })
}

/// Asking Explorer about every icon is the app's only recurring cost, so it only happens
/// when an icon could have been dragged: after input, with the desktop in the foreground.
fn icons_need_checking() -> bool {
    let mut input = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, ..Default::default() };
    let _ = unsafe { GetLastInputInfo(&mut input) }.ok();
    let active = desktop_is_active();
    with_app(|a| {
        a.ticks_since_check += 1;
        let used = input.dwTime != a.last_input && (active || a.desktop_was_active);
        let due = used || a.ticks_since_check >= TICKS_PER_FULL_CHECK;
        a.last_input = input.dwTime;
        a.desktop_was_active = active;
        if due {
            a.ticks_since_check = 0;
            a.worked_since_trim = true;
        }
        due
    })
}

/// Applies `theme.css` if it was saved since the last check.
fn reload_theme_if_changed(d: &Desktop) {
    let stamp = store::theme_stamp();
    if with_app(|a| a.theme_stamp) == stamp {
        return;
    }
    let theme = Rc::new(store::load_theme());
    // Loading recreates a deleted file, so read the stamp again.
    let stamp = store::theme_stamp();
    with_app(|a| {
        a.theme = theme;
        a.theme_stamp = stamp;
    });
    if let Ok(m) = read_metrics(d) {
        with_app(|a| a.metrics = Some(m));
    }
    place_all();
    for id in box_ids() {
        repaint(id);
    }
}

fn box_ids() -> Vec<u64> {
    with_app(|a| a.state.boxes.iter().map(|b| b.id).collect())
}

fn poll_inner() {
    let Some(d) = with_app(|a| a.desktop.clone()) else {
        connect();
        return;
    };
    if !unsafe { IsWindow(Some(d.hwnd())) }.as_bool() {
        connect();
        return;
    }
    // Hidden, nothing can be seen or dragged, so there is nothing to check.
    if with_app(|a| a.state.boxes.is_empty() || a.fade.hidden) {
        return;
    }
    // Explorer sometimes raises its icon list (showing icons, refreshing): the boxes would
    // still be there, just underneath. The topmost child is one cheap lookup.
    if unsafe { GetWindow(d.hwnd(), GW_CHILD) }.is_ok_and(|top| top == d.list()) {
        for (frame, tint) in all_box_windows() {
            raise(frame, tint);
        }
    }
    reload_theme_if_changed(&d);
    if !icons_need_checking() {
        return;
    }
    let result = (|| {
        prepare_desktop(&d)?;
        Ok::<_, windows::core::Error>((read_metrics(&d)?, d.icons()?))
    })();
    let (m, icons) = match result {
        Ok(v) => v,
        Err(e) => {
            store::log(&format!("lost the desktop, reconnecting: {e}"));
            with_app(|a| a.desktop = None);
            return;
        }
    };
    // An empty list is Explorer still loading, not every file having been deleted.
    if icons.is_empty() {
        return;
    }
    let view = d.hwnd();
    let outcome = with_app(|a| {
        a.metrics = Some(m);
        model::reconcile(&mut a.state.boxes, &icons, &m, &|r| area_of(view, r))
    });
    for id in &outcome.resized {
        sync_windows(*id);
    }
    if let Err(e) = d.apply(&outcome.moves) {
        store::log(&format!("could not move icons: {e}"));
    }
    if outcome.changed {
        save();
    }
}

// ---- Box actions ----

fn new_box() {
    set_hidden(false); // a new box should be seen
    let Some((d, m)) = with_app(|a| a.desktop.clone().zip(a.metrics)) else { return };
    let icons = d.icons().unwrap_or_default();
    let view = d.hwnd();
    let (width, height) = (m.cell_w * 4 + 2 * (m.border + m.pad), m.height_for(m.cell_w * 4, 8));
    let id = with_app(|a| {
        // Search the monitor under the cursor, where the user just clicked.
        let mut cursor = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cursor);
            let _ = ScreenToClient(view, &mut cursor);
        }
        let area = area_of(view, &Rect { x: cursor.x, y: cursor.y, w: 1, h: 1 });
        let rect = model::free_spot(&area, width, height, &icons, &a.state.boxes, &m);
        a.state.next_id += 1;
        let id = a.state.next_id;
        a.state.boxes.push(DesktopBox {
            id,
            name: format!("Box {}", id),
            rect,
            members: Vec::new(),
        });
        id
    });
    create_box_windows(id);
    if let Err(e) = prepare_desktop(&d) {
        store::log(&format!("could not prepare the desktop: {e}"));
    }
    save();
}

fn delete_box(id: u64) {
    let handles = with_app(|a| {
        a.state.boxes.retain(|b| b.id != id);
        a.rendered_size.remove(&id);
        a.windows.remove(&id)
    });
    if let Some((frame, tint)) = handles {
        unsafe {
            let _ = DestroyWindow(frame);
            let _ = DestroyWindow(tint);
        }
    }
    save();
}

fn restore_original() {
    unsafe {
        let answer = MessageBoxW(
            None,
            w!("This deletes all boxes and puts your icons back where they were before you first used Organizer.\n\nNo files are affected."),
            w!("Organizer"),
            MB_OKCANCEL | MB_ICONQUESTION | MB_SETFOREGROUND,
        );
        if answer != IDOK {
            return;
        }
    }
    destroy_box_windows();
    with_app(|a| a.state.boxes.clear());
    save();

    let Some(d) = with_app(|a| a.desktop.clone()) else { return };
    let Some(snapshot) = store::load_snapshot() else { return };
    let moves: Vec<model::Move> = snapshot
        .icons
        .into_iter()
        .map(|(key, x, y)| model::Move { key, x, y })
        .collect();
    let result = d
        .set_placement_flags(0)
        .and_then(|_| d.apply(&moves))
        .and_then(|_| d.set_placement_flags(snapshot.folder_flags));
    match result {
        // Keep the snapshot on failure, so the user can try again.
        Ok(()) => store::delete_snapshot(),
        Err(e) => store::log(&format!("could not restore the layout: {e}")),
    }
}

fn begin_rename(id: u64) {
    end_rename(true);
    let Some(b) = find_box(id) else { return };
    let Some((view, m)) = with_app(|a| a.desktop.as_ref().map(Desktop::hwnd).zip(a.metrics)) else { return };
    unsafe {
        let mut p = POINT { x: b.rect.x, y: b.rect.y };
        let _ = ClientToScreen(view, &mut p);
        let Ok(edit) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("EDIT"),
            &HSTRING::from(b.name.as_str()),
            WS_POPUP | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            p.x, p.y, b.rect.w, m.title,
            None, None, None, None,
        ) else { return };
        let font = GetStockObject(DEFAULT_GUI_FONT);
        SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        let old_proc = SetWindowLongPtrW(edit, GWLP_WNDPROC, rename_proc as *const () as isize);
        with_app(|a| a.rename = Some(Rename { id, edit, old_proc }));
        let _ = ShowWindow(edit, SW_SHOW);
        let _ = SetForegroundWindow(edit);
        let _ = SetFocus(Some(edit));
        SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
    }
}

fn end_rename(commit: bool) {
    // Taking it first makes this safe to re-enter: destroying the edit sends WM_KILLFOCUS.
    let Some(r) = with_app(|a| a.rename.take()) else { return };
    unsafe {
        // Unhook before reading: WM_GETTEXT must reach the edit control, because only it
        // holds the typed text. DefWindowProc would return the original name.
        SetWindowLongPtrW(r.edit, GWLP_WNDPROC, r.old_proc);
        let mut buf = [0u16; 128];
        let len = GetWindowTextW(r.edit, &mut buf) as usize;
        let name = String::from_utf16_lossy(&buf[..len]).trim().to_string();
        let _ = DestroyWindow(r.edit);
        if commit && !name.is_empty() {
            with_app(|a| {
                if let Some(b) = a.state.boxes.iter_mut().find(|b| b.id == r.id) {
                    b.name = name;
                }
            });
            save();
            repaint(r.id);
        }
    }
}

unsafe extern "system" fn rename_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let enter = VK_RETURN.0 as usize;
    let esc = VK_ESCAPE.0 as usize;
    match msg {
        WM_KEYDOWN if wp.0 == enter => {
            end_rename(true);
            return LRESULT(0);
        }
        WM_KEYDOWN if wp.0 == esc => {
            end_rename(false);
            return LRESULT(0);
        }
        // Swallowed so the edit control does not beep at them.
        WM_CHAR if wp.0 == 13 || wp.0 == 27 => return LRESULT(0),
        WM_KILLFOCUS => {
            end_rename(true);
            return LRESULT(0);
        }
        _ => {}
    }
    let old = with_app(|a| a.rename.as_ref().map(|r| r.old_proc));
    unsafe {
        match old {
            Some(p) => CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(p), hwnd, msg, wp, lp),
            None => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

// ---- Box windows ----

fn create_box_windows(id: u64) {
    let Some(view) = with_app(|a| a.desktop.as_ref().map(Desktop::hwnd)) else { return };
    let Some(b) = find_box(id) else { return };
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None).expect("module handle").into();
        let make = |class: PCWSTR, ex: WINDOW_EX_STYLE| -> Option<HWND> {
            let hwnd = CreateWindowExW(
                ex | WS_EX_LAYERED | WS_EX_NOACTIVATE,
                class,
                w!(""),
                WS_CHILD | WS_CLIPSIBLINGS,
                b.rect.x, b.rect.y, b.rect.w, b.rect.h,
                Some(view), None, Some(instance), None,
            )
            .map_err(|e| store::log(&format!("could not create a box window: {e}")))
            .ok()?;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, id as isize);
            Some(hwnd)
        };
        let Some(tint) = make(TINT_CLASS, WS_EX_TRANSPARENT) else { return };
        let Some(frame) = make(FRAME_CLASS, WINDOW_EX_STYLE(0)) else {
            let _ = DestroyWindow(tint);
            return;
        };
        with_app(|a| {
            a.windows.insert(id, (frame, tint));
            a.rendered_size.remove(&id);
        });
        sync_windows(id);
        if with_app(|a| a.fade.alpha > 0) {
            let _ = ShowWindow(tint, SW_SHOWNA);
            let _ = ShowWindow(frame, SW_SHOWNA);
        }
    }
}

// ---- Hiding the desktop ----

/// Toggles on a double-click on empty desktop space. Not on an icon (that opens it) and
/// not on a box's title bar (that renames the box).
fn on_desktop_double_click(lp: LPARAM) {
    let pt = POINT { x: loword(lp.0), y: hiword(lp.0) };
    let Some((d, m, hidden)) = with_app(|a| {
        let busy = a.drag.is_some() || a.rename.is_some();
        (!busy).then(|| Some((a.desktop.clone()?, a.metrics?, a.fade.hidden))).flatten()
    }) else {
        return;
    };
    let hit = unsafe { WindowFromPoint(pt) };
    if !watch::is_desktop(unsafe { GetAncestor(hit, GA_ROOT) }) {
        return;
    }
    if !hidden {
        if hit != d.hwnd() && hit != d.list() {
            return;
        }
        let mut p = pt;
        unsafe {
            let _ = ScreenToClient(d.hwnd(), &mut p);
        }
        // An icon's position is its image's corner; its clickable cell surrounds it.
        let on_icon = d.icons().map_or(true, |icons| {
            icons.iter().any(|i| {
                let (x, y) = (i.x - m.icon_x, i.y - m.icon_y);
                (x..x + m.cell_w).contains(&p.x) && (y..y + m.cell_h).contains(&p.y)
            })
        });
        if on_icon {
            return;
        }
    }
    set_hidden(!hidden);
}

/// Fades the boxes out or in. The icons switch at the start, so showing feels immediate.
fn set_hidden(hide: bool) {
    let Some((d, host)) = with_app(|a| {
        if a.fade.hidden == hide {
            return None;
        }
        a.fade.hidden = hide;
        a.fade.from = a.fade.alpha;
        a.fade.start = Some(std::time::Instant::now());
        Some((a.desktop.clone()?, a.host))
    }) else {
        return;
    };
    if let Err(e) = d.set_icons_visible(!hide) {
        store::log(&format!("could not {} the desktop icons: {e}", if hide { "hide" } else { "show" }));
    }
    if !hide {
        for (frame, tint) in all_box_windows() {
            raise(frame, tint);
            unsafe {
                let _ = ShowWindow(tint, SW_SHOWNA);
                let _ = ShowWindow(frame, SW_SHOWNA);
            }
        }
    }
    unsafe { SetTimer(Some(host), FADE_TIMER, FADE_FRAME_MS, None) };
    fade_step();
}

/// Puts a box above the icon list again: frame on top, its tint right below. Only the
/// stacking order changes.
fn raise(frame: HWND, tint: HWND) {
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
    unsafe {
        let _ = SetWindowPos(frame, Some(HWND_TOP), 0, 0, 0, 0, flags);
        let _ = SetWindowPos(tint, Some(frame), 0, 0, 0, 0, flags);
    }
}

fn all_box_windows() -> Vec<(HWND, HWND)> {
    with_app(|a| a.windows.values().copied().collect())
}

/// One frame of the fade. Only the windows' overall opacity changes, so nothing is redrawn.
fn fade_step() {
    let step = with_app(|a| {
        let f = &mut a.fade;
        let start = f.start?;
        let t = (start.elapsed().as_secs_f32() * 1000.0 / FADE_MS).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3); // ease-out: quick start, soft landing
        let to = if f.hidden { 0.0 } else { 255.0 };
        f.alpha = (f.from as f32 + (to - f.from as f32) * eased).round() as u8;
        let done = t >= 1.0;
        if done {
            f.start = None;
        }
        Some((f.alpha, done, f.hidden))
    });
    let host = with_app(|a| a.host);
    let Some((alpha, done, hidden)) = step else {
        unsafe {
            let _ = KillTimer(Some(host), FADE_TIMER);
        }
        return;
    };
    for (frame, tint) in all_box_windows() {
        if !hidden {
            // Showing the icons brings Explorer's icon list to the top, over the boxes, and
            // Explorer may do it a moment later. Raising every frame of the fade-in wins.
            raise(frame, tint);
        }
        set_opacity(frame, alpha);
        set_opacity(tint, alpha);
        if done && hidden {
            // Hidden windows cost the compositor nothing and can never catch a click.
            unsafe {
                let _ = ShowWindow(frame, SW_HIDE);
                let _ = ShowWindow(tint, SW_HIDE);
            }
        }
    }
    if done {
        unsafe {
            let _ = KillTimer(Some(host), FADE_TIMER);
        }
    }
}

fn blend(alpha: u8) -> BLENDFUNCTION {
    BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        SourceConstantAlpha: alpha,
        AlphaFormat: AC_SRC_ALPHA as u8,
        ..Default::default()
    }
}

/// Changes a layered window's overall opacity, keeping its pixels.
fn set_opacity(hwnd: HWND, alpha: u8) {
    unsafe {
        let _ = UpdateLayeredWindow(hwnd, None, None, None, None, None, COLORREF(0), Some(&blend(alpha)), ULW_ALPHA);
    }
}

fn destroy_box_windows() {
    let handles: Vec<_> = with_app(|a| {
        a.rendered_size.clear();
        a.windows.drain().map(|(_, h)| h).collect()
    });
    for (frame, tint) in handles {
        unsafe {
            let _ = DestroyWindow(frame);
            let _ = DestroyWindow(tint);
        }
    }
}

/// Theme lengths in device pixels for the current display scale.
fn sizes(t: &Theme, m: &Metrics, view: HWND) -> Sizes {
    let k = unsafe { GetDpiForWindow(view) }.max(96) as f32 / 96.0;
    Sizes {
        radius: t.radius * k,
        border: t.border_width * k,
        glow: t.glow_blur * k,
        title: m.title as f32,
        grip: m.grip(),
    }
}

/// Moves and sizes a box's windows, redrawing only if its size changed. A plain move is
/// just a window position, which keeps dragging cheap.
fn place_windows(id: u64, frame: HWND, tint: HWND, r: &Rect) {
    let Some((t, m, view, size)) = with_app(|a| {
        let view = a.desktop.as_ref()?.hwnd();
        Some((a.theme.clone(), a.metrics?, view, a.rendered_size.get(&id).copied()))
    }) else {
        return;
    };
    let margin = render::glow_margin(&sizes(&t, &m, view));
    unsafe {
        let flags = SWP_NOACTIVATE | SWP_NOREDRAW;
        let _ = SetWindowPos(frame, Some(HWND_TOP), r.x, r.y, r.w, r.h, flags);
        let _ = SetWindowPos(tint, Some(frame), r.x - margin, r.y - margin, r.w + 2 * margin, r.h + 2 * margin, flags);
    }
    if size != Some((r.w, r.h)) {
        repaint(id);
    }
}

fn sync_windows(id: u64) {
    let info = with_app(|a| {
        let handles = *a.windows.get(&id)?;
        let rect = a.state.boxes.iter().find(|b| b.id == id)?.rect;
        Some((handles, rect))
    });
    if let Some(((frame, tint), rect)) = info {
        place_windows(id, frame, tint, &rect);
    }
}

/// Redraws both layers of a box. Their pixels then live with the window manager, so
/// nothing stays allocated here and an idle box costs no CPU.
fn repaint(id: u64) {
    let Some(b) = find_box(id) else { return };
    let Some(((frame, tint), t, m, view)) = with_app(|a| {
        Some((*a.windows.get(&id)?, a.theme.clone(), a.metrics?, a.desktop.as_ref()?.hwnd()))
    }) else {
        return;
    };
    let s = sizes(&t, &m, view);
    let accent = t.accent;
    let (w, h) = (b.rect.w, b.rect.h);

    let mut top = render::frame_layer(&t, &s, accent, w, h);
    if let Some(mask) = title_mask(&b.name, &t, m.title, w, view) {
        render::blend_mask(&mut top, &mask, w, m.title, t.title_color.resolve(accent));
    }
    show_layer(frame, &top);
    drop(top);
    show_layer(tint, &render::tint_layer(&t, &s, accent, w, h));
    with_app(|a| a.rendered_size.insert(id, (w, h)));
}

/// Runs `f` with a memory DC holding a fresh top-down 32-bit bitmap of the given size.
fn with_bitmap<R>(w: i32, h: i32, f: impl FnOnce(HDC, &mut [u32]) -> R) -> Option<R> {
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = CreateCompatibleDC(None);
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return None;
        };
        let previous = SelectObject(dc, bitmap.into());
        let result = f(dc, std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize));
        SelectObject(dc, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        Some(result)
    }
}

fn show_layer(hwnd: HWND, layer: &Layer) {
    // Keeps the current fade, so a redraw mid-fade or while hidden does not pop in.
    let blend = blend(with_app(|a| a.fade.alpha));
    with_bitmap(layer.w, layer.h, |dc, px| unsafe {
        px.copy_from_slice(&layer.px);
        let size = SIZE { cx: layer.w, cy: layer.h };
        let origin = POINT::default();
        if let Err(e) = UpdateLayeredWindow(hwnd, None, None, Some(&size), Some(dc), Some(&origin), COLORREF(0), Some(&blend), ULW_ALPHA) {
            store::log(&format!("could not draw a box: {e}"));
        }
    });
}

/// Draws the title in white on black and returns its coverage, one byte per pixel.
/// Grayscale antialiasing, because ClearType's colored fringes need an opaque background.
fn title_mask(name: &str, t: &Theme, title_h: i32, w: i32, view: HWND) -> Option<Vec<u8>> {
    let dpi = unsafe { GetDpiForWindow(view) }.max(96) as f32;
    let px = |v: f32| (v * dpi / 96.0).round() as i32;
    let shown = if t.uppercase { name.to_uppercase() } else { name.to_string() };
    with_bitmap(w, title_h, |dc, bits| unsafe {
        let font = CreateFontW(
            -px(t.font_size), 0, 0, 0, t.font_weight, 0, 0, 0,
            DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, ANTIALIASED_QUALITY,
            0, &HSTRING::from(t.font_family.as_str()),
        );
        let previous = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00FFFFFF));
        SetTextCharacterExtra(dc, px(t.letter_spacing));
        let align = match t.text_align {
            Align::Left => DT_LEFT,
            Align::Center => DT_CENTER,
            Align::Right => DT_RIGHT,
        };
        let pad = px(t.title_padding);
        let mut rect = RECT { left: pad, top: 0, right: w - pad, bottom: title_h };
        let mut text: Vec<u16> = shown.encode_utf16().collect();
        DrawTextW(dc, &mut text, &mut rect, align | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
        SelectObject(dc, previous);
        let _ = DeleteObject(font.into());
        bits.iter().map(|p| ((p >> 8) & 0xFF) as u8).collect()
    })
}

fn hit_edges(x: i32, y: i32, r: &Rect, m: &Metrics) -> Edges {
    let grip = m.grip();
    Edges {
        left: x < grip && y >= m.title,
        right: x >= r.w - grip && y >= m.title,
        top: false,
        bottom: y >= r.h - grip,
    }
}

fn cursor_for(e: Edges) -> PCWSTR {
    match (e.left, e.right, e.bottom) {
        (true, _, true) => IDC_SIZENESW,
        (_, true, true) => IDC_SIZENWSE,
        (_, _, true) => IDC_SIZENS,
        (true, _, _) | (_, true, _) => IDC_SIZEWE,
        _ => IDC_SIZEALL,
    }
}

fn client_point(lp: LPARAM) -> POINT {
    POINT { x: loword(lp.0), y: hiword(lp.0) }
}

fn begin_drag(hwnd: HWND, lp: LPARAM) {
    let id = box_id(hwnd);
    let Some(b) = find_box(id) else { return };
    let Some((m, locked)) = with_app(|a| a.metrics.map(|m| (m, a.state.locked))) else { return };
    if locked {
        return;
    }
    let p = client_point(lp);
    let mut cursor = p;
    unsafe {
        let _ = ClientToScreen(hwnd, &mut cursor);
    }
    let edges = hit_edges(p.x, p.y, &b.rect, &m);
    if let Some(d) = with_app(|a| a.desktop.clone()) {
        if let Err(e) = d.refresh() {
            store::log(&format!("could not read icons: {e}"));
        }
    }
    with_app(|a| a.drag = Some(Drag { id, edges, start: b.rect, cursor }));
    unsafe { SetCapture(hwnd) };
}

fn continue_drag(hwnd: HWND, lp: LPARAM) {
    let info = with_app(|a| {
        let drag = a.drag.as_ref()?;
        Some((drag.start, drag.edges, drag.cursor, a.metrics?, *a.windows.get(&drag.id)?, drag.id))
    });
    let Some((start, edges, origin, m, (frame, tint), id)) = info else { return };
    let mut cursor = client_point(lp);
    unsafe {
        let _ = ClientToScreen(hwnd, &mut cursor);
    }
    let min_h = m.height_for(m.min_width(), 1);
    let rect = model::drag_rect(start, edges, cursor.x - origin.x, cursor.y - origin.y, m.min_width(), min_h);
    with_app(|a| {
        if let Some(b) = a.state.boxes.iter_mut().find(|b| b.id == id) {
            b.rect = rect;
        }
    });
    place_windows(id, frame, tint, &rect);

    // Asking Explorer to move icons is posted, so fast mouse movement collapses into
    // one move and the box itself never waits for Explorer.
    let host = with_app(|a| {
        let first = !a.follow_pending;
        a.follow_pending = true;
        first.then_some(a.host)
    });
    if let Some(host) = host {
        unsafe {
            let _ = PostMessageW(Some(host), WM_FOLLOW, WPARAM(0), LPARAM(0));
        }
    }
}

/// Keeps the dragged box's icons in their slots while it moves.
fn follow_icons() {
    let info = with_app(|a| {
        a.follow_pending = false;
        let id = a.drag.as_ref()?.id;
        let b = a.state.boxes.iter().find(|b| b.id == id)?;
        let m = a.metrics?;
        let moves: Vec<model::Move> = b
            .members
            .iter()
            .enumerate()
            .map(|(i, key)| {
                let (x, y) = m.slot(&b.rect, i);
                model::Move { key: key.clone(), x, y }
            })
            .collect();
        Some((a.desktop.clone()?, moves))
    });
    if let Some((d, moves)) = info {
        if let Err(e) = d.follow(&moves) {
            store::log(&format!("could not move icons: {e}"));
        }
    }
}

fn end_drag() {
    if with_app(|a| a.drag.take()).is_some() {
        place_all();
    }
}

unsafe extern "system" fn frame_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_LBUTTONDOWN => begin_drag(hwnd, lp),
        WM_MOUSEMOVE => continue_drag(hwnd, lp),
        WM_LBUTTONUP => unsafe {
            let _ = ReleaseCapture();
            end_drag();
        },
        // Capture lost another way, such as Alt-Tab mid-drag: settle where the box is.
        WM_CAPTURECHANGED => end_drag(),
        WM_LBUTTONDBLCLK => begin_rename(box_id(hwnd)),
        WM_RBUTTONUP => show_box_menu(box_id(hwnd)),
        WM_SETCURSOR => unsafe {
            let id = box_id(hwnd);
            let cursor = match (find_box(id), with_app(|a| a.metrics.map(|m| (m, a.state.locked)))) {
                (Some(b), Some((m, false))) => {
                    let mut p = POINT::default();
                    let _ = GetCursorPos(&mut p);
                    let _ = ScreenToClient(hwnd, &mut p);
                    cursor_for(hit_edges(p.x, p.y, &b.rect, &m))
                }
                _ => IDC_ARROW,
            };
            SetCursor(LoadCursorW(None, cursor).ok());
            return LRESULT(1);
        },
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}

unsafe extern "system" fn tint_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    thread_local! {
        static TASKBAR_CREATED: u32 = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    }
    if msg == TASKBAR_CREATED.with(|m| *m) {
        // Explorer restarted: the tray icon and every box window are gone.
        add_tray_icon();
        connect();
        return LRESULT(0);
    }
    match msg {
        WM_TIMER if wp.0 == POLL_TIMER => poll(),
        WM_TIMER if wp.0 == UPDATE_TIMER => {
            // Replaces the one-minute first timer with the regular interval.
            unsafe { SetTimer(Some(hwnd), UPDATE_TIMER, UPDATE_EVERY_MS, None) };
            start_update_check(false);
        }
        WM_TIMER if wp.0 == FADE_TIMER => fade_step(),
        WM_DESKTOP_DOUBLE_CLICK => on_desktop_double_click(lp),
        WM_UPDATE => on_update_checked(wp.0 != 0, unsafe { update::take(lp) }),
        WM_UPDATE_DOWNLOADED => on_update_downloaded(unsafe { update::take(lp) }),
        WM_EXIT => unsafe { PostQuitMessage(0) },
        WM_FOLLOW => follow_icons(),
        WM_NEW_BOX => new_box(),
        WM_TRAY if matches!(lp.0 as u32, WM_LBUTTONUP | WM_RBUTTONUP) => show_tray_menu(),
        WM_TRAY if lp.0 as u32 == NIN_BALLOONUSERCLICK => install_update(),
        WM_DISPLAYCHANGE => place_all(),
        WM_DESTROY => unsafe { PostQuitMessage(0) },
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    LRESULT(0)
}
