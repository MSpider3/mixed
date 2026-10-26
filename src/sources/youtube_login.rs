//! YouTube Music sign-in through a browser window that only mixed uses.
//!
//! mixed opens Brave, Chrome, Chromium, Vivaldi or Firefox with a profile of its own,
//! at Google's sign-in page. Once the window is closed, the cookies of that profile
//! are read with yt-dlp. A copied browser cookie stops working within days because
//! the browser it came from keeps replacing it; nothing else uses this profile, so
//! its cookies stay valid much longer. Signing in again opens the same profile,
//! where Google usually still knows the account.

use std::path::{Path, PathBuf};

/// Where Google's sign-in sends the browser once done.
const SIGN_IN_URL: &str = "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F";

/// How browsers keep their profile, which decides the command line and how yt-dlp
/// reads the cookies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Chromium,
    Firefox,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Browser {
    program: PathBuf,
    family: Family,
}

/// Browsers tried in this order when the configuration names none.
const KNOWN_BROWSERS: &[&str] = &[
    "brave-browser",
    "brave",
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "vivaldi",
    "vivaldi-stable",
    "firefox",
    "librewolf",
];

fn family_of(program: &Path) -> Family {
    let name = program
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.contains("firefox") || name.contains("librewolf") || name.contains("zen") {
        Family::Firefox
    } else {
        Family::Chromium
    }
}

/// `name` as found on PATH, or as given when it is a path to a program.
fn find_program(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The browser named in the configuration (`youtube_browser`), else the first known one.
fn find_browser(configured: Option<&str>) -> Result<Browser, String> {
    let program = match configured.filter(|c| !c.trim().is_empty()) {
        Some(name) => find_program(name.trim())
            .ok_or_else(|| format!("The browser \"{}\" set in config.json was not found", name))?,
        None => KNOWN_BROWSERS
            .iter()
            .find_map(|name| find_program(name))
            .ok_or_else(|| {
                "No supported browser found (Brave, Chrome, Chromium, Vivaldi or Firefox). \
                 Set \"youtube_browser\" in config.json, or press c to paste a cookie."
                    .to_string()
            })?,
    };
    Ok(Browser {
        family: family_of(&program),
        program,
    })
}

/// The browser profile that only mixed uses.
fn profile_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "mixed")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir)
        .join("youtube-browser")
}

fn browser_command(browser: &Browser, profile: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(&browser.program);
    match browser.family {
        Family::Chromium => {
            cmd.arg(format!("--user-data-dir={}", profile.display()))
                // Cookies encrypted with a fixed key that yt-dlp knows, instead of
                // one kept in the desktop's keyring
                .arg("--password-store=basic")
                .args([
                    "--no-first-run",
                    "--no-default-browser-check",
                    "--new-window",
                ]);
        }
        Family::Firefox => {
            cmd.arg("--profile").arg(profile).arg("--no-remote");
        }
    }
    cmd.arg(SIGN_IN_URL)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

/// Turn a Netscape cookie file into a `Cookie` header for YouTube.
fn cookie_header(netscape: &str) -> String {
    netscape
        .lines()
        .filter_map(|line| {
            // "#HttpOnly_" marks HttpOnly cookies; other lines starting with # are comments
            let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
            if line.starts_with('#') {
                return None;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let [domain, _, _, _, _, name, value] = fields[..] else {
                return None;
            };
            (domain.trim_start_matches('.') == "youtube.com" || domain.ends_with(".youtube.com"))
                .then(|| format!("{}={}", name, value))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The cookies of a signed-in Google session carry one of these.
fn has_account_cookie(header: &str) -> bool {
    header
        .split("; ")
        .any(|c| c.starts_with("SAPISID=") || c.starts_with("__Secure-3PAPISID="))
}

/// Read the YouTube cookies of the profile with yt-dlp.
async fn read_cookies(browser: &Browser, profile: &Path, yt_dlp: &str) -> Result<String, String> {
    let spec = match browser.family {
        Family::Chromium => format!("chromium:{}", profile.display()),
        Family::Firefox => format!("firefox:{}", profile.display()),
    };
    let out = profile.with_file_name("youtube-cookies.txt");
    let _ = std::fs::remove_file(&out);
    // yt-dlp writes the cookie file even though it cannot open the placeholder URL,
    // which keeps it from making any request
    let result = tokio::process::Command::new(yt_dlp)
        .args(["--cookies-from-browser", &spec, "--cookies"])
        .arg(&out)
        .args(["--skip-download", "about:blank"])
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("Could not run yt-dlp to read the cookies: {}", e))?;
    let text = std::fs::read_to_string(&out);
    let _ = std::fs::remove_file(&out);
    let Ok(text) = text else {
        let stderr = String::from_utf8_lossy(&result.stderr);
        log::warn!("yt-dlp could not read the browser cookies: {}", stderr);
        return Err("Could not read the cookies of the sign-in window".to_string());
    };
    Ok(cookie_header(&text))
}

/// Whether YouTube Music's page says this marks a signed-in session.
fn logged_in_marker(page: &str) -> Option<bool> {
    if page.contains("\"LOGGED_IN\":true") {
        Some(true)
    } else if page.contains("\"LOGGED_IN\":false") {
        Some(false)
    } else {
        None
    }
}

/// Ask YouTube Music whether `cookie` is a signed-in session. `None` when it could
/// not be asked (offline): then nothing is known either way.
pub async fn is_signed_in(cookie: &str) -> Option<bool> {
    let page = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .ok()?
        .get("https://music.youtube.com/")
        .header("Cookie", cookie)
        .header(
            "User-Agent",
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0 Safari/537.36",
        )
        .header("Accept-Language", "en")
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    logged_in_marker(&page)
}

/// Open the sign-in window, wait for it to be closed and return the session's
/// cookie. `progress` tells the user what the sign-in waits for.
pub async fn sign_in(
    configured_browser: Option<&str>,
    yt_dlp: &str,
    progress: impl Fn(&str),
) -> Result<String, String> {
    let browser = find_browser(configured_browser)?;
    let profile = profile_dir();
    std::fs::create_dir_all(&profile)
        .map_err(|e| format!("Cannot create {}: {}", profile.display(), e))?;
    log::info!(
        "YouTube sign-in: opening {} with profile {}",
        browser.program.display(),
        profile.display()
    );

    let started = std::time::Instant::now();
    let mut child = browser_command(&browser, &profile)
        .spawn()
        .map_err(|e| format!("Could not open {}: {}", browser.program.display(), e))?;
    progress("A browser window opened: sign in to YouTube Music there, then close the window.");
    let _ = child.wait().await;
    // A window that closes at once was handed to a browser already running this
    // profile: the sign-in goes on there
    if started.elapsed() < std::time::Duration::from_secs(3) {
        return Err(
            "The sign-in window is already open: finish there, close it, then press Enter again."
                .to_string(),
        );
    }

    progress("Reading the sign-in…");
    let cookie = read_cookies(&browser, &profile, yt_dlp).await?;
    if !has_account_cookie(&cookie) {
        return Err(
            "The window was closed before signing in. Press Enter to open it again.".to_string(),
        );
    }
    if is_signed_in(&cookie).await == Some(false) {
        return Err(
            "YouTube Music does not accept this sign-in. Press Enter to open the window again."
                .to_string(),
        );
    }
    Ok(cookie)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_cookies_become_a_header() {
        let file = "# Netscape HTTP Cookie File\n\
                    .youtube.com\tTRUE\t/\tTRUE\t1893456000\tSAPISID\tabc/def\n\
                    #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1893456000\t__Secure-3PSID\txyz\n\
                    .google.com\tTRUE\t/\tTRUE\t1893456000\tSID\tnot-for-youtube\n\
                    music.youtube.com\tFALSE\t/\tFALSE\t0\tPREF\tf6=80\n";
        let header = cookie_header(file);
        assert_eq!(header, "SAPISID=abc/def; __Secure-3PSID=xyz; PREF=f6=80");
        assert!(has_account_cookie(&header));
        assert!(!has_account_cookie("VISITOR_INFO1_LIVE=a; YSC=b"));
    }

    #[test]
    fn the_page_tells_whether_the_session_is_signed_in() {
        assert_eq!(
            logged_in_marker(r#"ytcfg.set({"LOGGED_IN":true,"#),
            Some(true)
        );
        assert_eq!(logged_in_marker(r#"{"LOGGED_IN":false}"#), Some(false));
        assert_eq!(logged_in_marker("<html>"), None);
    }

    #[test]
    fn browsers_get_the_command_line_of_their_family() {
        assert_eq!(
            family_of(Path::new("/usr/bin/brave-browser")),
            Family::Chromium
        );
        assert_eq!(family_of(Path::new("google-chrome")), Family::Chromium);
        assert_eq!(family_of(Path::new("/usr/bin/firefox")), Family::Firefox);
        let browser = Browser {
            program: "brave".into(),
            family: Family::Chromium,
        };
        let cmd = browser_command(&browser, Path::new("/p"));
        let args: Vec<_> = cmd.as_std().get_args().map(|a| a.to_owned()).collect();
        assert!(args.iter().any(|a| a == "--user-data-dir=/p"));
        assert!(args.iter().any(|a| a == "--password-store=basic"));
    }
}
