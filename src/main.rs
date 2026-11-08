#![windows_subsystem = "windows"]

mod desktop;
mod model;
mod render;
mod store;
mod theme;
mod ui;
mod update;

use windows::Win32::System::{
    Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
    Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE},
};

fn main() {
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().expect("COM init") };
    if std::env::args().nth(1).as_deref() == Some("list") {
        // A GUI-subsystem exe has no console. Attaching replaces handles, so keep a redirect.
        if unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }.map_or(true, |h| h.is_invalid() || h.0.is_null()) {
            let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
        }
        return list();
    }
    // Sent by the desktop's right-click menu entry.
    let new_box = std::env::args().nth(1).as_deref() == Some("new");
    if new_box && ui::request_new_box() {
        return;
    }
    ui::run(new_box);
    unsafe { CoUninitialize() };
}

/// Read-only diagnostic: prints what Organizer can see, and changes nothing.
fn list() {
    let d = match desktop::Desktop::connect() {
        Ok(d) => d,
        Err(e) => return println!("cannot reach the desktop: {e}"),
    };
    println!("view: {:?}", d.hwnd());
    let theme = store::load_theme();
    println!("metrics: {:?}", d.metrics(theme.title_height, theme.border_width, theme.padding));
    println!("flags: {:?}", d.folder_flags().map(|f| format!("{f:#x}")));
    match d.icons() {
        Ok(icons) => {
            for i in icons {
                println!("{:>5},{:<5} {}", i.x, i.y, i.key);
            }
        }
        Err(e) => println!("cannot read icons: {e}"),
    }
}
