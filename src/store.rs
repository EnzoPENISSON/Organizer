//! Saved boxes and the safety snapshot of the user's original icon layout.

use crate::{model::DesktopBox, theme::{self, Theme}};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::PathBuf, time::SystemTime};

#[derive(Serialize, Deserialize)]
pub struct State {
    pub next_id: u64,
    pub locked: bool,
    /// Double-clicking empty desktop space hides the boxes and icons. On unless turned off,
    /// including for files saved before the setting existed.
    #[serde(default = "on")]
    pub double_click_hides: bool,
    pub boxes: Vec<DesktopBox>,
}

impl Default for State {
    fn default() -> Self {
        Self { next_id: 0, locked: false, double_click_hides: true, boxes: Vec::new() }
    }
}

fn on() -> bool {
    true
}

/// The layout as it was before Organizer first changed anything.
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    pub folder_flags: u32,
    pub icons: Vec<(String, i32, i32)>,
}

fn dir() -> PathBuf {
    let base = std::env::var_os("ORGANIZER_DIR").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Organizer")
    });
    let _ = fs::create_dir_all(&base);
    base
}

fn write_atomic(name: &str, text: &str) -> std::io::Result<()> {
    let tmp = dir().join(format!("{name}.tmp"));
    fs::write(&tmp, text)?;
    fs::rename(tmp, dir().join(name))
}

pub fn load() -> State {
    let path = dir().join("boxes.json");
    let Ok(text) = fs::read_to_string(&path) else { return State::default() };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        // Keep the unreadable file: the next save would otherwise destroy it.
        log(&format!("boxes.json is unreadable, keeping it as boxes.json.bad: {e}"));
        let _ = fs::rename(&path, dir().join("boxes.json.bad"));
        State::default()
    })
}

pub fn save(state: &State) {
    let result = serde_json::to_string_pretty(state)
        .map_err(std::io::Error::other)
        .and_then(|text| write_atomic("boxes.json", &text));
    if let Err(e) = result {
        log(&format!("could not save boxes.json: {e}"));
    }
}

pub fn theme_path() -> PathBuf {
    dir().join("theme.css")
}

/// When `theme.css` last changed, so the app can react without rereading it.
pub fn theme_stamp() -> Option<SystemTime> {
    fs::metadata(theme_path()).and_then(|m| m.modified()).ok()
}

/// Reads `theme.css`, creating it from the default if missing. Problems are logged,
/// and whatever parsed still applies, so a typo never breaks the boxes.
pub fn load_theme() -> Theme {
    let path = theme_path();
    if !path.exists() {
        if let Err(e) = fs::write(&path, theme::DEFAULT_CSS) {
            log(&format!("could not create theme.css: {e}"));
        }
        return Theme::default();
    }
    match fs::read_to_string(&path) {
        Ok(css) => {
            let (theme, warnings) = theme::parse(&css, Theme::default());
            for w in warnings {
                log(&format!("theme.css: {w}"));
            }
            theme
        }
        Err(e) => {
            log(&format!("could not read theme.css: {e}"));
            Theme::default()
        }
    }
}

pub fn has_snapshot() -> bool {
    dir().join("original-layout.json").exists()
}

pub fn save_snapshot(s: &Snapshot) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(s).map_err(std::io::Error::other)?;
    write_atomic("original-layout.json", &text)
}

pub fn load_snapshot() -> Option<Snapshot> {
    serde_json::from_str(&fs::read_to_string(dir().join("original-layout.json")).ok()?).ok()
}

pub fn delete_snapshot() {
    let _ = fs::remove_file(dir().join("original-layout.json"));
}

pub fn log(message: &str) {
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir().join("organizer.log")) {
        let _ = writeln!(f, "{message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_saved_before_the_double_click_setting_keep_it_on() {
        let state: State = serde_json::from_str(r#"{"next_id":0,"locked":false,"boxes":[]}"#).unwrap();
        assert!(state.double_click_hides);
        assert!(State::default().double_click_hides);
    }
}
