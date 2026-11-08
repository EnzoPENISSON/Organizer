//! Checks GitHub for a newer release than the running exe.
//!
//! Kept cheap on purpose: one small HTTPS request through Windows' own WinHTTP (no extra
//! crates), made on a short-lived thread that exits right after, a minute after start and
//! then twice a day. Nothing runs between checks, and a failed check just waits for the next.

use serde::Deserialize;
use std::ffi::c_void;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        Networking::WinHttp::*,
        UI::WindowsAndMessaging::PostMessageW,
    },
    core::{PCWSTR, w},
};

const API_HOST: PCWSTR = w!("api.github.com");
const LATEST_PATH: PCWSTR = w!("/repos/EnzoPENISSON/Organizer/releases/latest");
const USER_AGENT: PCWSTR = w!("Organizer-update-check");
/// Where the download link may point. Anything else falls back to the releases page.
const RELEASES_URL: &str = "https://github.com/EnzoPENISSON/Organizer/releases";
const MAX_RESPONSE: usize = 1 << 20;

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

pub enum Outcome {
    Newer { tag: String, url: String },
    UpToDate,
    Failed(String),
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
}

/// Compares GitHub's "latest release" answer with `current`.
fn outcome(json: &[u8], current: Version) -> Outcome {
    let release: Release = match serde_json::from_slice(json) {
        Ok(r) => r,
        Err(e) => return Outcome::Failed(format!("unexpected answer from GitHub: {e}")),
    };
    match parse_tag(&release.tag_name) {
        Some(latest) if latest > current => {
            // The link is opened with the shell, so only ever follow it to this repo's releases.
            let url = if release.html_url.starts_with(&format!("{RELEASES_URL}/")) {
                release.html_url
            } else {
                RELEASES_URL.to_string()
            };
            Outcome::Newer { tag: release.tag_name, url }
        }
        Some(_) => Outcome::UpToDate,
        None => Outcome::Failed(format!("unrecognized release tag {:?}", release.tag_name)),
    }
}

/// Checks on a background thread, then posts `msg` to `notify` with `wparam` = `manual`
/// and `lparam` = a boxed [`Outcome`], to be claimed with [`take`]. False if no thread
/// could be started, in which case nothing will be posted.
pub fn check_async(notify: HWND, msg: u32, manual: bool) -> bool {
    let target = notify.0 as isize; // HWND is not Send; the handle itself is fine to share
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let result = match fetch_latest() {
                Ok(Some(body)) => outcome(&body, current()),
                Ok(None) => Outcome::UpToDate, // no release published yet
                Err(e) => Outcome::Failed(e),
            };
            let ptr = Box::into_raw(Box::new(result));
            let posted = unsafe {
                PostMessageW(Some(HWND(target as *mut c_void)), msg, WPARAM(manual as usize), LPARAM(ptr as isize))
            };
            if posted.is_err() {
                drop(unsafe { Box::from_raw(ptr) });
            }
        })
        .is_ok()
}

/// Claims the outcome posted by [`check_async`].
///
/// # Safety
/// `lp` must be the lparam of a message posted by `check_async`, claimed only once.
pub unsafe fn take(lp: LPARAM) -> Outcome {
    *unsafe { Box::from_raw(lp.0 as *mut Outcome) }
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

/// The body of GitHub's latest-release answer, or None if the repo has no release yet.
fn fetch_latest() -> Result<Option<Vec<u8>>, String> {
    let fail = |step: &'static str| move |e: windows::core::Error| format!("{step}: {}", e.message());
    unsafe {
        let session = handle(
            WinHttpOpen(USER_AGENT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0),
            "open",
        )?;
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 15_000);
        let connection = handle(WinHttpConnect(session.0, API_HOST, INTERNET_DEFAULT_HTTPS_PORT, 0), "connect")?;
        let request = handle(
            WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                LATEST_PATH,
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ),
            "request",
        )?;
        let headers: Vec<u16> =
            "Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n".encode_utf16().collect();
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
        let mut buf = [0u8; 8192];
        loop {
            let mut read = 0u32;
            WinHttpReadData(request.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut read)
                .map_err(fail("read"))?;
            if read == 0 {
                return Ok(Some(body));
            }
            body.extend_from_slice(&buf[..read as usize]);
            if body.len() > MAX_RESPONSE {
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

    fn release(tag: &str, url: &str) -> Vec<u8> {
        format!(r#"{{"tag_name":"{tag}","html_url":"{url}","body":"notes","assets":[]}}"#).into_bytes()
    }

    #[test]
    fn reports_newer_release() {
        let url = "https://github.com/EnzoPENISSON/Organizer/releases/tag/v0.1.0-build.13";
        match outcome(&release("v0.1.0-build.13", url), [0, 1, 0, 12]) {
            Outcome::Newer { tag, url: u } => assert_eq!((tag.as_str(), u.as_str()), ("v0.1.0-build.13", url)),
            _ => panic!("expected a newer release"),
        }
    }

    #[test]
    fn same_or_older_is_up_to_date() {
        let url = "https://github.com/EnzoPENISSON/Organizer/releases/tag/x";
        assert!(matches!(outcome(&release("v0.1.0-build.12", url), [0, 1, 0, 12]), Outcome::UpToDate));
        assert!(matches!(outcome(&release("v0.1.0-build.3", url), [0, 1, 0, 12]), Outcome::UpToDate));
    }

    #[test]
    fn never_links_outside_the_repo() {
        match outcome(&release("v9.0.0", "https://example.com/evil.exe"), [0, 1, 0, 0]) {
            Outcome::Newer { url, .. } => assert_eq!(url, RELEASES_URL),
            _ => panic!("expected a newer release"),
        }
    }

    /// Talks to GitHub, so it only runs on request: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_check_reaches_github() {
        match fetch_latest() {
            Ok(Some(body)) => assert!(!matches!(outcome(&body, current()), Outcome::Failed(_))),
            Ok(None) => {} // no release published yet
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn bad_answers_fail() {
        assert!(matches!(outcome(b"not json", [0, 1, 0, 0]), Outcome::Failed(_)));
        assert!(matches!(outcome(&release("latest", RELEASES_URL), [0, 1, 0, 0]), Outcome::Failed(_)));
    }
}
