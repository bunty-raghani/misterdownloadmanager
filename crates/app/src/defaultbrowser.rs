//! The default browser of this computer and its version (#668), found once when MDM starts, on a
//! thread of its own (reading a version can start a program: `--version`, bounded in time): its
//! User-Agent becomes the one MDM sends for links added by hand (`mdm_engine::agent`). Where to
//! look is each system's (`win32::default_browser`, `unix::default_browser`); reading what they
//! find (a command line, a `.desktop` file, a version's output) is here, the same everywhere.

use std::sync::mpsc;
use std::time::Duration;

use mdm_engine::agent::Found;

/// Look for the default browser on a thread of its own; the automatic User-Agent is a current
/// Chrome's until it is found. Returns a receiver told when it is done (`wait`).
pub fn start() -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("default-browser".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            let found = find();
            let browser = found.as_ref().map_or("none MDM knows".to_owned(), |f| {
                format!("{:?} {}", f.browser, f.version.as_deref().unwrap_or("?"))
            });
            mdm_engine::agent::set_automatic(found);
            tracing::info!(
                "default browser: {browser} ({} ms); links added by hand go as {:?}",
                started.elapsed().as_millis(),
                mdm_engine::agent::automatic()
            );
            let _ = tx.send(());
        });
    if spawned.is_err() {
        mdm_engine::agent::set_automatic(None);
    }
    rx
}

/// Wait at most `limit` for `start`'s search (the command line's downloads begin at once).
pub fn wait(done: &mpsc::Receiver<()>, limit: Duration) {
    let _ = done.recv_timeout(limit);
}

fn find() -> Option<Found> {
    #[cfg(windows)]
    return crate::win32::default_browser();
    #[cfg(not(windows))]
    return crate::unix::default_browser();
}

/// The program a Windows `shell\open\command` runs: `"C:\…\brave.exe" --single-argument %1`
/// (quoted), or `C:\Program Files\Mozilla Firefox\firefox.exe -osint -url "%1"` (not quoted: up
/// to its `.exe`).
// Windows' registry only (win32.rs); tested everywhere.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_program(command: &str) -> Option<String> {
    let c = command.trim();
    let program = match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => {
            let end = c.to_ascii_lowercase().find(".exe")? + 4;
            &c[..end]
        }
    };
    let program = program.trim();
    (!program.is_empty()).then(|| program.to_owned())
}

/// The program a `.desktop` file's `Exec=` line runs: its first word that isn't `env` or a
/// variable it sets (`env MOZ_X=1 /usr/bin/firefox %u` → `/usr/bin/firefox`), quotes removed.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn exec_program(exec: &str) -> Option<String> {
    let mut words = shell_words(exec).into_iter();
    loop {
        let w = words.next()?;
        if w == "env" || (w.contains('=') && !w.starts_with('/')) {
            continue;
        }
        return Some(w);
    }
}

/// `line`'s words as a `.desktop` file's Exec splits them: by spaces, with "double quotes".
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn shell_words(line: &str) -> Vec<String> {
    let (mut words, mut word, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            '\\' if quoted => word.extend(chars.next()),
            c if c.is_whitespace() && !quoted => {
                if any || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                any = false;
            }
            c => word.push(c),
        }
    }
    if any || !word.is_empty() {
        words.push(word);
    }
    words
}

/// The default handler of `x-scheme-handler/https` in a `mimeapps.list` (its first `.desktop`).
#[cfg(any(target_os = "linux", test))]
pub fn mimeapps_https(text: &str) -> Option<String> {
    crate::xdgicons::ini_value(text, "Default Applications", "x-scheme-handler/https")?
        .split(';')
        .map(str::trim)
        .find(|d| !d.is_empty())
        .map(str::to_owned)
}

/// The version a browser's `--version` prints: its first word that is a dotted number
/// ("Google Chrome 157.0.8103.38" → "157.0.8103.38", "Mozilla Firefox 160.0" → "160.0").
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn version_in(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|w| {
            w.contains('.')
                && w.starts_with(|c: char| c.is_ascii_digit())
                && w.split('.')
                    .next()
                    .is_some_and(|m| m.parse::<u32>().is_ok())
        })
        .map(|w| w.trim_end_matches([',', ';']).to_owned())
}

/// The bundle ID of the app macOS opens `scheme` links with, from LaunchServices' handlers as
/// `defaults read com.apple.LaunchServices/com.apple.launchservices.secure LSHandlers` prints
/// them; None when none is set (Safari, the system's own).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn ls_handler(text: &str, scheme: &str) -> Option<String> {
    // Each handler is a dictionary one level in; its preferred versions, one deeper, are not
    // its keys.
    let (mut depth, mut role, mut matched) = (0usize, None::<String>, false);
    for line in text.lines().map(str::trim) {
        let at = depth;
        if at == 2
            && let Some((k, v)) = line.trim_end_matches(';').split_once('=')
        {
            let v = v.trim().trim_matches('"').to_owned();
            match k.trim() {
                "LSHandlerURLScheme" => matched = v.eq_ignore_ascii_case(scheme),
                "LSHandlerRoleAll" => role = Some(v),
                _ => {}
            }
        }
        depth += line.matches(['{', '(']).count();
        let closes = line.matches(['}', ')']).count();
        if closes > 0 && depth.saturating_sub(closes) < 2 && depth >= 2 {
            if matched && let Some(r) = role.take().filter(|r| r != "-" && !r.is_empty()) {
                return Some(r);
            }
            (role, matched) = (None, false);
        }
        depth = depth.saturating_sub(closes);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_commands_name_their_program() {
        assert_eq!(
            windows_program(
                r#""C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe" --single-argument %1"#
            )
            .as_deref(),
            Some(r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe")
        );
        assert_eq!(
            windows_program(r#"C:\Program Files\Mozilla Firefox\firefox.EXE -osint -url "%1""#)
                .as_deref(),
            Some(r"C:\Program Files\Mozilla Firefox\firefox.EXE")
        );
        assert_eq!(windows_program(""), None);
        assert_eq!(windows_program("rundll32 url.dll"), None);
    }

    #[test]
    fn desktop_files_name_their_program() {
        let entry = "[Desktop Entry]\nName=Google Chrome\nExec=/usr/bin/google-chrome-stable %U\n\
                     [Desktop Action new-window]\nExec=/usr/bin/google-chrome-stable --new-window\n";
        let exec = crate::xdgicons::ini_value(entry, "Desktop Entry", "Exec").unwrap();
        assert_eq!(
            exec_program(&exec).as_deref(),
            Some("/usr/bin/google-chrome-stable")
        );
        assert_eq!(
            exec_program("env BAMF_DESKTOP_FILE_HINT=/x.desktop /snap/bin/firefox %u").as_deref(),
            Some("/snap/bin/firefox")
        );
        assert_eq!(
            exec_program(r#""/opt/My Browser/browser" %U"#).as_deref(),
            Some("/opt/My Browser/browser")
        );
        assert_eq!(
            crate::xdgicons::ini_value(
                "[App]\nVendor=Mozilla\nVersion=155.0.1\n",
                "App",
                "Version"
            )
            .as_deref(),
            Some("155.0.1")
        );
        let list = "[Added Associations]\nx-scheme-handler/https=a.desktop;\n\
                    [Default Applications]\ntext/html=firefox_firefox.desktop\n\
                    x-scheme-handler/https=com.google.Chrome.desktop;firefox_firefox.desktop;\n";
        assert_eq!(
            mimeapps_https(list).as_deref(),
            Some("com.google.Chrome.desktop")
        );
    }

    #[test]
    fn versions_from_what_browsers_print() {
        for (out, v) in [
            ("Google Chrome 157.0.8103.38 \n", "157.0.8103.38"),
            ("Mozilla Firefox 160.0", "160.0"),
            ("Chromium 142.0.7444.175 snap", "142.0.7444.175"),
            ("Brave Browser 157.1.99.55", "157.1.99.55"),
            ("122.0.5643.24", "122.0.5643.24"),
        ] {
            assert_eq!(version_in(out).as_deref(), Some(v), "{out}");
        }
        assert_eq!(version_in("Opening in existing browser session."), None);
    }

    #[test]
    fn macos_https_handler() {
        let handlers = r#"(
        {
        LSHandlerContentType = "public.html";
        LSHandlerPreferredVersions =         {
            LSHandlerRoleAll = "-";
        };
        LSHandlerRoleAll = "com.apple.safari";
    },
        {
        LSHandlerPreferredVersions =         {
            LSHandlerRoleAll = "-";
        };
        LSHandlerRoleAll = "com.google.chrome";
        LSHandlerURLScheme = https;
    },
        {
        LSHandlerRoleAll = "org.mozilla.firefox";
        LSHandlerURLScheme = http;
    }
)"#;
        assert_eq!(
            ls_handler(handlers, "https").as_deref(),
            Some("com.google.chrome")
        );
        assert_eq!(
            ls_handler(handlers, "http").as_deref(),
            Some("org.mozilla.firefox")
        );
        assert_eq!(ls_handler(handlers, "ftp"), None);
        assert_eq!(ls_handler("", "https"), None);
    }
}
