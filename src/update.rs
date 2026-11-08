//! Checks GitHub for a newer release than the running exe, and downloads its installer.
//!
//! Kept cheap on purpose: one small HTTPS request through Windows' own WinHTTP (no extra
//! crates), made on a short-lived thread that exits right after, a minute after start and
//! then twice a day. Nothing runs between checks, and a failed check just waits for the next.
//! The installer (a few MB) is only downloaded when the user chooses to install.

use serde::Deserialize;
use std::{ffi::c_void, path::PathBuf};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        Networking::WinHttp::*,
        UI::WindowsAndMessaging::PostMessageW,
    },
    core::{HSTRING, PCWSTR, w},
};

const API_HOST: &str = "api.github.com";
const LATEST_PATH: &str = "/repos/EnzoPENISSON/Organizer/releases/latest";
const USER_AGENT: PCWSTR = w!("Organizer-update-check");
/// Links from GitHub's answer are only followed inside this repo's releases.
const RELEASES_URL: &str = "https://github.com/EnzoPENISSON/Organizer/releases";
const DOWNLOAD_URL: &str = "https://github.com/EnzoPENISSON/Organizer/releases/download/";
/// The same files through the API. It redirects straight to GitHub's file storage, so it
/// still works when github.com's download links time out (HTTP 504), which happens.
const ASSET_API_URL: &str = "https://api.github.com/repos/EnzoPENISSON/Organizer/releases/assets/";
/// Download attempts over all links, with a growing pause between rounds.
const DOWNLOAD_ROUNDS: u64 = 3;
/// Release asset names: `Organizer-Setup-<tag>.exe` (see installer/organizer.iss).
const INSTALLER_PREFIX: &str = "Organizer-Setup-";
const MAX_ANSWER: usize = 1 << 20;
const MAX_INSTALLER: usize = 64 << 20;

/// The CI run number (see build.rs and the release workflow). Absent in local builds.
const BUILD: Option<&str> = option_env!("ORGANIZER_BUILD");

/// major, minor, patch, build. Compares in that order.
pub type Version = [u32; 4];

/// The running exe's version, matching the release tags `v<version>-build.<run number>`.
pub fn current() -> Version {
    parse_tag(&format!("v{}-build.{}", env!("CARGO_PKG_VERSION"), BUILD.unwrap_or("0"))).unwrap_or_default()
}

/// False for local builds: they have no build number to compare, so they do not check
/// on their own (a manual check still works).
pub fn is_release_build() -> bool {
    BUILD.is_some()
}

/// Shown in the tray menu, e.g. "Organizer 0.1.0 (build 12)".
pub fn label() -> String {
    match BUILD {
        Some(b) => format!("Organizer {} (build {b})", env!("CARGO_PKG_VERSION")),
        None => format!("Organizer {} (local build)", env!("CARGO_PKG_VERSION")),
    }
}

/// Parses `v1.2.3-build.45` or `v1.2.3` (build 0).
pub fn parse_tag(tag: &str) -> Option<Version> {
    let rest = tag.strip_prefix('v').unwrap_or(tag);
    let (version, build) = match rest.split_once("-build.") {
        Some((v, b)) => (v, b.parse().ok()?),
        None => (rest, 0),
    };
    let mut parts = version.split('.').map(|p| p.parse::<u32>().ok());
    let v = [parts.next()??, parts.next()??, parts.next()??, build];
    parts.next().is_none().then_some(v)
}

/// A newer release than the running exe.
#[derive(Clone)]
pub struct Release {
    pub tag: String,
    /// The release's web page.
    pub page: String,
    /// Links to its installer, best first. Empty if the release has none.
    pub installer: Vec<String>,
}

pub enum Outcome {
    Newer(Release),
    UpToDate,
    Failed(String),
}

#[derive(Deserialize)]
struct Answer {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    /// The API link, which serves the file with `Accept: application/octet-stream`.
    #[serde(default)]
    url: String,
    browser_download_url: String,
}

/// Compares GitHub's "latest release" answer with `current`.
fn outcome(json: &[u8], current: Version) -> Outcome {
    let answer: Answer = match serde_json::from_slice(json) {
        Ok(a) => a,
        Err(e) => return Outcome::Failed(format!("unexpected answer from GitHub: {e}")),
    };
    match parse_tag(&answer.tag_name) {
        Some(latest) if latest > current => {
            // These links are opened or downloaded, so only ever follow them into this repo.
            let page = if answer.html_url.starts_with(&format!("{RELEASES_URL}/")) {
                answer.html_url
            } else {
                RELEASES_URL.to_string()
            };
            let installer = answer
                .assets
                .into_iter()
                .find(|a| a.name.starts_with(INSTALLER_PREFIX) && a.name.ends_with(".exe"))
                .map(|a| {
                    let api = Some(a.url).filter(|u| u.starts_with(ASSET_API_URL));
                    let direct = Some(a.browser_download_url).filter(|u| u.starts_with(DOWNLOAD_URL));
                    api.into_iter().chain(direct).collect()
                })
                .unwrap_or_default();
            Outcome::Newer(Release { tag: answer.tag_name, page, installer })
        }
        Some(_) => Outcome::UpToDate,
        None => Outcome::Failed(format!("unrecognized release tag {:?}", answer.tag_name)),
    }
}

/// Runs `work` on a short-lived thread, then posts `msg` to `notify` with `wparam` and
/// `lparam` = the boxed result, to be claimed with [`take`]. False if no thread could be
/// started, in which case nothing will be posted.
fn run_async<T: Send + 'static>(notify: HWND, msg: u32, wparam: usize, work: impl FnOnce() -> T + Send + 'static) -> bool {
    let target = notify.0 as isize; // HWND is not Send; the handle itself is fine to share
    std::thread::Builder::new()
        .name("update".into())
        .spawn(move || {
            let ptr = Box::into_raw(Box::new(work()));
            let posted =
                unsafe { PostMessageW(Some(HWND(target as *mut c_void)), msg, WPARAM(wparam), LPARAM(ptr as isize)) };
            if posted.is_err() {
                drop(unsafe { Box::from_raw(ptr) });
            }
        })
        .is_ok()
}

/// Checks for a newer release. Posts an [`Outcome`], with `wparam` = `manual`.
pub fn check_async(notify: HWND, msg: u32, manual: bool) -> bool {
    run_async(notify, msg, manual as usize, || match get(API_HOST, LATEST_PATH, Accept::Json, MAX_ANSWER) {
        Ok(Some(body)) => outcome(&body, current()),
        Ok(None) => Outcome::UpToDate, // no release published yet
        Err(e) => Outcome::Failed(e),
    })
}

/// Downloads `release`'s installer to the temp folder. Posts a `Result<PathBuf, String>`.
pub fn download_async(notify: HWND, msg: u32, release: &Release) -> bool {
    if release.installer.is_empty() {
        return false;
    }
    let links = release.installer.clone();
    // The tag passed parse_tag, but keep the file name plain whatever it holds.
    let safe_tag: String = release.tag.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')).collect();
    let path = std::env::temp_dir().join(format!("{INSTALLER_PREFIX}{safe_tag}.exe"));
    run_async(notify, msg, 0, move || -> Result<PathBuf, String> {
        let body = download(&links)?;
        std::fs::write(&path, body).map_err(|e| format!("could not save the installer: {e}"))?;
        Ok(path)
    })
}

/// Tries each link in turn, a few rounds, pausing longer each time: GitHub's download
/// servers sometimes time out for a while. Returns the last error if all fail.
fn download(links: &[String]) -> Result<Vec<u8>, String> {
    let mut last = String::from("no download link");
    for round in 0..DOWNLOAD_ROUNDS {
        if round > 0 {
            std::thread::sleep(std::time::Duration::from_secs(3 * round));
        }
        for link in links {
            let Some((host, path)) = link.strip_prefix("https://").and_then(|rest| rest.find('/').map(|i| rest.split_at(i)))
            else {
                continue;
            };
            match get(host, path, Accept::Binary, MAX_INSTALLER) {
                Ok(Some(body)) => return Ok(body),
                Ok(None) => last = "the installer is missing from the release".into(),
                Err(e) => last = e,
            }
        }
    }
    Err(last)
}

/// Claims a result posted by [`check_async`] (`T` = [`Outcome`]) or [`download_async`]
/// (`T` = `Result<PathBuf, String>`).
///
/// # Safety
/// `lp` must be the lparam of a message posted by the matching function, claimed only once.
pub unsafe fn take<T>(lp: LPARAM) -> T {
    *unsafe { Box::from_raw(lp.0 as *mut T) }
}

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn handle(h: *mut c_void, step: &str) -> Result<Handle, String> {
    if h.is_null() {
        Err(format!("{step}: {}", std::io::Error::last_os_error()))
    } else {
        Ok(Handle(h))
    }
}

#[derive(Clone, Copy)]
enum Accept {
    /// An API answer as JSON.
    Json,
    /// A file. The API's asset links need this to send the file instead of its details.
    Binary,
}

/// An HTTPS GET. None on 404. Redirects (GitHub serves downloads from another host) are
/// followed by WinHTTP, which never follows one from HTTPS down to HTTP.
fn get(host: &str, path: &str, accept: Accept, max: usize) -> Result<Option<Vec<u8>>, String> {
    let fail = |step: &'static str| move |e: windows::core::Error| format!("{step}: {}", e.message());
    unsafe {
        let session = handle(
            WinHttpOpen(USER_AGENT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0),
            "open",
        )?;
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 30_000);
        let connection =
            handle(WinHttpConnect(session.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0), "connect")?;
        let request = handle(
            WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                &HSTRING::from(path),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ),
            "request",
        )?;
        let headers: Vec<u16> = match accept {
            Accept::Json => "Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n",
            Accept::Binary => "Accept: application/octet-stream\r\n",
        }
        .encode_utf16()
        .collect();
        WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0).map_err(fail("send"))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(fail("receive"))?;

        let (mut status, mut size) = (0u32, size_of::<u32>() as u32);
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(fail("status"))?;
        match status {
            200 => {}
            404 => return Ok(None),
            s => return Err(format!("GitHub answered HTTP {s}")),
        }

        let mut body = Vec::new();
        let mut buf = [0u8; 16384];
        loop {
            let mut read = 0u32;
            WinHttpReadData(request.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut read)
                .map_err(fail("read"))?;
            if read == 0 {
                return Ok(Some(body));
            }
            body.extend_from_slice(&buf[..read as usize]);
            if body.len() > max {
                return Err("answer from GitHub is too large".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags() {
        assert_eq!(parse_tag("v0.1.0-build.12"), Some([0, 1, 0, 12]));
        assert_eq!(parse_tag("v1.2.3"), Some([1, 2, 3, 0]));
        assert_eq!(parse_tag("2.0.1-build.7"), Some([2, 0, 1, 7]));
        assert_eq!(parse_tag("v1.2"), None);
        assert_eq!(parse_tag("v1.2.3.4"), None);
        assert_eq!(parse_tag("v1.2.3-build.x"), None);
        assert_eq!(parse_tag("nightly"), None);
    }

    #[test]
    fn orders_versions_before_builds() {
        assert!(parse_tag("v0.2.0-build.1") > parse_tag("v0.1.9-build.99"));
        assert!(parse_tag("v0.1.0-build.13") > parse_tag("v0.1.0-build.12"));
        assert!(parse_tag("v0.10.0") > parse_tag("v0.9.0"));
    }

    const PAGE: &str = "https://github.com/EnzoPENISSON/Organizer/releases/tag/v0.1.0-build.13";
    const SETUP: &str =
        "https://github.com/EnzoPENISSON/Organizer/releases/download/v0.1.0-build.13/Organizer-Setup-v0.1.0-build.13.exe";
    const SETUP_API: &str = "https://api.github.com/repos/EnzoPENISSON/Organizer/releases/assets/723586684";

    /// Assets are (name, API link, direct link).
    fn answer(tag: &str, page: &str, assets: &[(&str, &str, &str)]) -> Vec<u8> {
        let assets: Vec<_> = assets
            .iter()
            .map(|(name, api, url)| serde_json::json!({ "name": name, "url": api, "browser_download_url": url, "size": 1 }))
            .collect();
        serde_json::json!({ "tag_name": tag, "html_url": page, "body": "notes", "assets": assets })
            .to_string()
            .into_bytes()
    }

    fn newer(json: &[u8]) -> Release {
        match outcome(json, [0, 1, 0, 12]) {
            Outcome::Newer(r) => r,
            _ => panic!("expected a newer release"),
        }
    }

    #[test]
    fn reports_newer_release_and_its_installer_api_link_first() {
        let r = newer(&answer(
            "v0.1.0-build.13",
            PAGE,
            &[
                ("organizer.exe", "", "https://github.com/EnzoPENISSON/Organizer/releases/download/x/organizer.exe"),
                ("Organizer-Setup-v0.1.0-build.13.exe", SETUP_API, SETUP),
            ],
        ));
        assert_eq!((r.tag.as_str(), r.page.as_str()), ("v0.1.0-build.13", PAGE));
        assert_eq!(r.installer, [SETUP_API, SETUP]);
    }

    #[test]
    fn release_without_installer_still_counts() {
        assert!(newer(&answer("v0.1.0-build.13", PAGE, &[])).installer.is_empty());
    }

    #[test]
    fn same_or_older_is_up_to_date() {
        assert!(matches!(outcome(&answer("v0.1.0-build.12", PAGE, &[]), [0, 1, 0, 12]), Outcome::UpToDate));
        assert!(matches!(outcome(&answer("v0.1.0-build.3", PAGE, &[]), [0, 1, 0, 12]), Outcome::UpToDate));
    }

    #[test]
    fn never_follows_links_outside_the_repo() {
        let r = newer(&answer(
            "v9.0.0",
            "https://example.com/page",
            &[("Organizer-Setup-v9.0.0.exe", "https://example.com/api/evil", "https://example.com/evil.exe")],
        ));
        assert_eq!(r.page, RELEASES_URL);
        assert!(r.installer.is_empty());
    }

    #[test]
    fn bad_answers_fail() {
        assert!(matches!(outcome(b"not json", [0, 1, 0, 0]), Outcome::Failed(_)));
        assert!(matches!(outcome(&answer("latest", PAGE, &[]), [0, 1, 0, 0]), Outcome::Failed(_)));
    }

    /// Talks to GitHub, so it only runs on request: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_check_reaches_github() {
        match get(API_HOST, LATEST_PATH, Accept::Json, MAX_ANSWER) {
            Ok(Some(body)) => assert!(!matches!(outcome(&body, current()), Outcome::Failed(_))),
            Ok(None) => {} // no release published yet
            Err(e) => panic!("{e}"),
        }
    }

    /// Downloads the latest installer the way the updater does: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_download_gets_the_installer() {
        let body = get(API_HOST, LATEST_PATH, Accept::Json, MAX_ANSWER).unwrap().expect("a release");
        let Outcome::Newer(release) = outcome(&body, [0, 0, 0, 0]) else { panic!("expected a release") };
        let setup = download(&release.installer[..1]).unwrap(); // the API link alone
        assert!(setup.len() > 100_000 && setup.starts_with(b"MZ"), "not an exe: {} bytes", setup.len());
    }
}
