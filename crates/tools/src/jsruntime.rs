//! The JavaScript runtime yt-dlp solves YouTube's challenges with (#1570, #1448, #1470): without
//! one, a YouTube look-up is slow, lists few qualities, or fails ("Requested format is not
//! available"). yt-dlp supports Deno, Node.js, QuickJS and Bun, in that order of priority, and
//! is told exactly which one to use (`--no-js-runtimes --js-runtimes <name>:<path>`).
//!
//! Which one, first found wins ([`current`]):
//! 1. the one the person chose in Helpers (**Use another program...**, `chosen`; credit: the
//!    idea of choosing your own runtime is YairDies', #1541);
//! 2. one already on the computer, in yt-dlp's order (`proc::system_js_runtimes`), once it has
//!    answered with a version yt-dlp can use (Deno 2.3, Node.js 22, Bun 1.2.11 or newer; QuickJS
//!    once it runs) and, for Node.js, one its project still supports ([`node_end_of_life`]: past
//!    its end of life MDM's own, kept up to date, is used instead). The trade-off: a supported
//!    Node.js of the computer's is used as it is, updated by whoever installed it;
//! 3. MDM's own Node.js (`node`), downloaded and checked against Node.js's signed checksums;
//! 4. MDM's own Deno, which earlier versions of MDM installed: used until its Node.js is in
//!    place, then removed ([`retire_deno`]). A Deno the person installed is never touched.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::{Error, Installed};

/// A JavaScript runtime yt-dlp supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Deno,
    Node,
    QuickJs,
    Bun,
}

impl Kind {
    /// yt-dlp's order of priority, highest first.
    pub const ALL: [Kind; 4] = [Kind::Deno, Kind::Node, Kind::QuickJs, Kind::Bun];

    /// Its name to yt-dlp (`--js-runtimes <name>:<path>`).
    pub fn ytdlp_name(self) -> &'static str {
        match self {
            Kind::Deno => "deno",
            Kind::Node => "node",
            Kind::QuickJs => "quickjs",
            Kind::Bun => "bun",
        }
    }

    /// Its name for people (a program's name: never translated).
    pub fn label(self) -> &'static str {
        match self {
            Kind::Deno => "Deno",
            Kind::Node => "Node.js",
            Kind::QuickJs => "QuickJS",
            Kind::Bun => "Bun",
        }
    }

    /// The oldest version yt-dlp can use (its EJS guide); None: not asked (QuickJS has no
    /// version option in every build).
    pub fn oldest(self) -> Option<&'static str> {
        match self {
            Kind::Deno => Some("2.3.0"),
            Kind::Node => Some("22.0.0"),
            Kind::Bun => Some("1.2.11"),
            Kind::QuickJs => None,
        }
    }

    fn of_name(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.ytdlp_name() == name)
    }

    /// Which runtime a program is, by its file's name (`node.exe`, `deno`, `qjs`, `bun`, or a
    /// name that has one of them in it: `node22.exe`).
    pub fn of_program(exe: &Path) -> Option<Kind> {
        let stem = exe.file_stem()?.to_string_lossy().to_ascii_lowercase();
        if stem.contains("deno") {
            Some(Kind::Deno)
        } else if stem.contains("node") {
            Some(Kind::Node)
        } else if stem.contains("qjs") || stem.contains("quickjs") {
            Some(Kind::QuickJs)
        } else if stem.contains("bun") {
            Some(Kind::Bun)
        } else {
            None
        }
    }
}

/// The version a runtime prints when asked (`--version`): Deno `deno 2.5.6 (stable, …)`,
/// Node.js `v26.13.1`, Bun `1.3.2`; None for anything else.
pub fn parse_version(kind: Kind, out: &str) -> Option<String> {
    let first = out.lines().map(str::trim).find(|l| !l.is_empty())?;
    let v = match kind {
        Kind::Deno => first.strip_prefix("deno ")?.split_whitespace().next()?,
        Kind::Node => first.strip_prefix('v')?,
        Kind::Bun => first.split(['+', '-']).next()?,
        Kind::QuickJs => return None,
    };
    let numbers = v.split('.').count() >= 2
        && v.split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    (numbers && v.len() <= 32).then(|| v.to_owned())
}

/// Whether `version` of `kind` is one yt-dlp can use ([`Kind::oldest`]).
pub fn old_enough(kind: Kind, version: &str) -> bool {
    kind.oldest()
        .is_none_or(|oldest| oldest == version || crate::newer(version, oldest))
}

/// Where a runtime comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The person chose it in Helpers.
    Chosen,
    /// It was on the computer already.
    System,
    /// MDM's own copy (Node.js, or the Deno earlier versions installed).
    Own,
}

/// The runtime yt-dlp is told of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runtime {
    pub kind: Kind,
    pub exe: PathBuf,
    /// As it answered (None: QuickJS, which isn't asked).
    pub version: Option<String>,
    pub from: Origin,
}

impl Runtime {
    /// What yt-dlp is told: only this one, whatever else it would find itself. The path is
    /// passed as it is, not as text (a name that isn't valid Unicode stays whole).
    pub fn args(&self) -> Vec<OsString> {
        let mut named = OsString::from(format!("{}:", self.kind.ytdlp_name()));
        named.push(self.exe.as_os_str());
        vec!["--no-js-runtimes".into(), "--js-runtimes".into(), named]
    }
}

/// A program asked ([`ask`]), as what, and its answer ([`ask_now`]'s).
type Asked = (PathBuf, Kind, Option<Option<String>>);

/// The answers of the programs asked this session ([`ask`]).
static ASKED: Mutex<Vec<Asked>> = Mutex::new(Vec::new());

/// How many programs were asked for their version ([`ask_now`]): tests check a window asks none
/// on its own thread.
static PROBES: AtomicU64 = AtomicU64::new(0);

/// [`PROBES`]' count.
pub fn probes() -> u64 {
    PROBES.load(Ordering::Relaxed)
}

/// `exe`'s answer as `kind`, if it was asked this session.
fn asked(kind: Kind, exe: &Path) -> Option<Option<Option<String>>> {
    ASKED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(p, k, _)| p == exe && *k == kind)
        .map(|(_, _, v)| v.clone())
}

/// Ask `exe` (a `kind`) for its version, once a session for each program (a program updated
/// meanwhile keeps the version it said). Err: it didn't answer as that runtime, is too old for
/// yt-dlp, or (Node.js) is past its end of life. Runs the program: never on a window's thread.
pub fn ask(kind: Kind, exe: &Path) -> Result<Option<String>, Error> {
    let answer = match asked(kind, exe) {
        Some(a) => a,
        None => {
            let a = ask_now(kind, exe);
            ASKED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((exe.to_owned(), kind, a.clone()));
            a
        }
    };
    judge(kind, answer, today())
}

/// Whether a computer's runtime that answered `answer` (as [`ask_now`] gives it) is one to use
/// on `day` (days since 1970).
fn judge(kind: Kind, answer: Option<Option<String>>, day: u64) -> Result<Option<String>, Error> {
    let Some(version) = answer else {
        return Err(Error::NotTheProgram(kind.label()));
    };
    if let (Some(v), Some(oldest)) = (&version, kind.oldest())
        && !old_enough(kind, v)
    {
        return Err(Error::TooOld {
            name: kind.label(),
            version: v.clone(),
            oldest,
        });
    }
    if kind == Kind::Node
        && let Some(v) = &version
        && let Some(next) = node_end_of_life(v, day)
    {
        return Err(Error::TooOld {
            name: kind.label(),
            version: v.clone(),
            oldest: next,
        });
    }
    Ok(version)
}

/// Days since 1970, today.
fn today() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400)
}

/// Days since 1970 of a date.
const fn days(y: i64, m: i64, d: i64) -> u64 {
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as u64
}

/// When each Node.js major line reaches its end of life (nodejs.org's release schedule, as of
/// this MDM release): even lines are long-term supported for 30 months, odd ones for 8. Past it,
/// a computer's Node.js of that line is passed over (MDM's own, kept to a supported release, is
/// used); the line to move to is returned, for the message. A line not listed (newer than this
/// MDM) is taken as supported.
fn node_end_of_life(version: &str, day: u64) -> Option<&'static str> {
    let major: u64 = version.split('.').next()?.parse().ok()?;
    let (end, next) = match major {
        0..22 => return None, // too old for yt-dlp already (Kind::oldest)
        22 => (days(2027, 4, 30), "24.0.0"),
        23 => (days(2025, 6, 1), "24.0.0"),
        24 => (days(2028, 4, 30), "26.0.0"),
        25 => (days(2026, 6, 1), "26.0.0"),
        26 => (days(2029, 4, 30), "28.0.0"),
        27 => (days(2027, 6, 1), "28.0.0"),
        _ => return None,
    };
    (day >= end).then_some(next)
}

/// [`ask`], not remembered: `Some(version)` when it answered as `kind` (QuickJS: when it ran a
/// line of JavaScript, its version not known).
fn ask_now(kind: Kind, exe: &Path) -> Option<Option<String>> {
    PROBES.fetch_add(1, Ordering::Relaxed);
    let mut cmd = std::process::Command::new(exe);
    match kind {
        Kind::QuickJs => cmd.args(["-e", "print('mdm-js-ok')"]),
        _ => cmd.arg("--version"),
    };
    let out = crate::proc::output(cmd, kind.ytdlp_name(), Duration::from_secs(20)).ok()?;
    let out = String::from_utf8_lossy(&out);
    match kind {
        Kind::QuickJs => out.contains("mdm-js-ok").then_some(None),
        _ => parse_version(kind, &out).map(Some),
    }
}

/// The runtime the person chose in Helpers, if any and still there.
pub fn chosen(dir: &Path) -> Option<Runtime> {
    let c = crate::chosen::chosen_runtime(dir)?;
    Some(Runtime {
        kind: Kind::of_name(&c.kind)?,
        exe: c.exe,
        version: (!c.version.is_empty()).then_some(c.version),
        from: Origin::Chosen,
    })
}

/// Use `exe` as the runtime from now on, once it has answered as one yt-dlp can use.
pub fn choose(dir: &Path, exe: &Path) -> Result<Runtime, Error> {
    let kind = Kind::of_program(exe).ok_or(Error::NotTheProgram(ANY))?;
    let version = ask_now(kind, exe).ok_or(Error::NotTheProgram(kind.label()))?;
    if let (Some(v), Some(oldest)) = (&version, kind.oldest())
        && !old_enough(kind, v)
    {
        return Err(Error::TooOld {
            name: kind.label(),
            version: v.clone(),
            oldest,
        });
    }
    crate::chosen::choose_runtime(
        dir,
        crate::chosen::ChosenRuntime {
            kind: kind.ytdlp_name().to_owned(),
            version: version.clone().unwrap_or_default(),
            exe: exe.to_owned(),
        },
    )?;
    Ok(Runtime {
        kind,
        exe: exe.to_owned(),
        version,
        from: Origin::Chosen,
    })
}

/// The runtimes yt-dlp supports, by name, for a message.
pub const ANY: &str = "Node.js, Deno, Bun or QuickJS";

/// Back to the runtime MDM finds or installs.
pub fn forget(dir: &Path) -> Result<(), Error> {
    crate::chosen::forget_runtime(dir)
}

/// The first of `found` (the computer's runtimes, in yt-dlp's order) that answers as a runtime
/// yt-dlp can use, asked with `ask` (`(kind, exe) -> version`).
pub(crate) fn first_usable(
    found: Vec<(Kind, PathBuf)>,
    ask: impl Fn(Kind, &Path) -> Result<Option<String>, Error>,
) -> Option<Runtime> {
    found
        .into_iter()
        .find_map(|(kind, exe)| match ask(kind, &exe) {
            Ok(version) => Some(Runtime {
                kind,
                exe,
                version,
                from: Origin::System,
            }),
            Err(e) => {
                tracing::info!("JavaScript runtime {} passed over: {e}", exe.display());
                None
            }
        })
}

/// The computer's own runtime, MDM's aside, for yt-dlp at `ytdlp` (a runtime beside MDM's own
/// yt-dlp counts too, per yt-dlp's EJS guide).
pub fn system(ytdlp: Option<&Path>) -> Option<Runtime> {
    first_usable(crate::proc::system_js_runtimes(ytdlp), ask)
}

/// MDM's own runtime in the tools folder `dir`: its Node.js, else the Deno an earlier version
/// installed; either only in a version yt-dlp can use ([`old_enough`]).
pub fn own(dir: &Path) -> Option<Runtime> {
    let of = |kind, i: Installed| {
        let usable = old_enough(kind, &i.version);
        usable.then_some(Runtime {
            kind,
            exe: i.exe,
            version: Some(i.version),
            from: Origin::Own,
        })
    };
    crate::node::installed(dir)
        .and_then(|i| of(Kind::Node, i))
        .or_else(|| crate::deno::installed(dir).and_then(|i| of(Kind::Deno, i)))
}

/// The runtime yt-dlp at `ytdlp` uses, in the order this module's header gives; `dir`: the
/// tools folder (none: the computer's own only). May run the computer's runtimes ([`ask`]):
/// never on a window's thread ([`current_now`] is).
pub fn current(dir: Option<&Path>, ytdlp: Option<&Path>) -> Option<Runtime> {
    pick(dir.and_then(chosen), || system(ytdlp), || dir.and_then(own))
}

/// [`current`] when it can be told without running any program (a runtime chosen, none on the
/// computer, or every one there already asked this session): `Some(answer)`. None: a program
/// would have to be asked first; ask [`current`] off the window's thread.
pub fn current_now(dir: Option<&Path>, ytdlp: Option<&Path>) -> Option<Option<Runtime>> {
    if let Some(c) = dir.and_then(chosen) {
        return Some(Some(c));
    }
    for (kind, exe) in crate::proc::system_js_runtimes(ytdlp) {
        match judge(kind, asked(kind, &exe)?, today()) {
            Ok(version) => {
                return Some(Some(Runtime {
                    kind,
                    exe,
                    version,
                    from: Origin::System,
                }));
            }
            Err(_) => continue,
        }
    }
    Some(dir.and_then(own))
}

/// [`current`] from what was found: `chosen`, `system`, `own`, in that order.
pub(crate) fn pick(
    chosen: Option<Runtime>,
    system: impl FnOnce() -> Option<Runtime>,
    own: impl FnOnce() -> Option<Runtime>,
) -> Option<Runtime> {
    chosen.or_else(system).or_else(own)
}

/// Whether MDM installs a runtime of its own here (Node.js has a build for this system and
/// processor).
pub fn available() -> bool {
    crate::node::available()
}

/// Install MDM's own Node.js (or update it), then remove the Deno an earlier version of MDM
/// installed, which it replaces.
pub async fn install_latest(
    dir: &Path,
    net: &mdm_engine::Options,
    progress: Option<&crate::update::Progress>,
) -> Result<Installed, Error> {
    let node = crate::node::install_latest(dir, net, progress).await?;
    retire_deno(dir);
    Ok(node)
}

/// Remove MDM's own Deno (`<tools>/deno`, a folder only MDM makes, never a Deno the person
/// installed, which lives elsewhere) once MDM's own Node.js is in place and no run uses that
/// Deno. Left for later (the next start or check) while one does.
pub fn retire_deno(dir: &Path) -> bool {
    if crate::node::installed(dir).is_none() {
        return false;
    }
    // The person chose that very Deno as their runtime (Use another program...): theirs now.
    if chosen(dir).is_some_and(|c| c.exe.starts_with(dir.join("deno"))) {
        return false;
    }
    crate::deno::remove_own(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_as_each_runtime_prints_them() {
        let deno =
            "deno 2.5.6 (stable, release, x86_64-pc-windows-msvc)\nv8 14.0\ntypescript 5.9\n";
        assert_eq!(parse_version(Kind::Deno, deno).as_deref(), Some("2.5.6"));
        assert_eq!(
            parse_version(Kind::Node, "v26.13.1\n").as_deref(),
            Some("26.13.1")
        );
        assert_eq!(
            parse_version(Kind::Bun, "1.3.2\n").as_deref(),
            Some("1.3.2")
        );
        assert_eq!(
            parse_version(Kind::Bun, "1.2.20+6ad208bc3\n").as_deref(),
            Some("1.2.20")
        );
        // Another program's answer.
        assert_eq!(parse_version(Kind::Node, "Python 3.13.1\n"), None);
        assert_eq!(parse_version(Kind::Deno, "v26.13.1\n"), None);
        assert_eq!(parse_version(Kind::Node, "vlc 3.0\n"), None);
        assert_eq!(parse_version(Kind::Bun, "Usage: x\n"), None);
        assert_eq!(parse_version(Kind::Node, ""), None);
    }

    #[test]
    fn old_runtimes_are_told_apart() {
        assert!(old_enough(Kind::Node, "22.0.0"));
        assert!(old_enough(Kind::Node, "26.13.1"));
        assert!(!old_enough(Kind::Node, "20.18.1"));
        assert!(!old_enough(Kind::Node, "21.99.0"));
        assert!(old_enough(Kind::Deno, "2.3.0"));
        assert!(!old_enough(Kind::Deno, "2.2.9"));
        assert!(!old_enough(Kind::Deno, "1.46.3"));
        assert!(!old_enough(Kind::Bun, "1.1.38"));
        assert!(old_enough(Kind::QuickJs, "anything"));
    }

    #[test]
    fn a_program_is_known_by_its_name() {
        for (name, kind) in [
            ("node.exe", Some(Kind::Node)),
            ("/usr/bin/node", Some(Kind::Node)),
            ("nodejs", Some(Kind::Node)),
            ("Node22.EXE", Some(Kind::Node)),
            ("deno.exe", Some(Kind::Deno)),
            ("qjs", Some(Kind::QuickJs)),
            ("quickjs-ng.exe", Some(Kind::QuickJs)),
            ("bun", Some(Kind::Bun)),
            ("python.exe", None),
            ("yt-dlp.exe", None),
        ] {
            assert_eq!(Kind::of_program(Path::new(name)), kind, "{name}");
        }
    }

    /// yt-dlp is told of exactly one runtime, the others it would find cleared first; a Windows
    /// path's `C:` stays whole (yt-dlp splits at the first colon).
    #[test]
    fn yt_dlp_is_told_of_one_runtime() {
        let r = Runtime {
            kind: Kind::Node,
            exe: PathBuf::from(
                r"C:\Users\A B\AppData\Roaming\misterdownloadmanager\tools\node\26.13.1\node.exe",
            ),
            version: Some("26.13.1".into()),
            from: Origin::Own,
        };
        assert_eq!(
            r.args(),
            [
                OsString::from("--no-js-runtimes"),
                OsString::from("--js-runtimes"),
                OsString::from(format!("node:{}", r.exe.display()))
            ]
        );
    }

    fn rt(kind: Kind, exe: &str, from: Origin) -> Runtime {
        Runtime {
            kind,
            exe: PathBuf::from(exe),
            version: None,
            from,
        }
    }

    /// The order: the person's choice, then the computer's, then MDM's own; a system runtime
    /// that is too old or doesn't answer is passed over for the next one in yt-dlp's order.
    #[test]
    fn the_runtime_is_chosen_in_order() {
        let chosen = rt(Kind::Bun, "/mine/bun", Origin::Chosen);
        let system = rt(Kind::Node, "/usr/bin/node", Origin::System);
        let own = rt(Kind::Node, "/tools/node/26.13.1/node", Origin::Own);
        let never = || -> Option<Runtime> { panic!("not looked for") };
        assert_eq!(
            pick(Some(chosen.clone()), never, never),
            Some(chosen.clone())
        );
        assert_eq!(
            pick(None, || Some(system.clone()), never),
            Some(system.clone())
        );
        assert_eq!(pick(None, || None, || Some(own.clone())), Some(own));
        assert_eq!(pick(None, || None, || None), None);

        let found = vec![
            (Kind::Deno, PathBuf::from("/old/deno")),
            (Kind::Node, PathBuf::from("/nvm/v20/node")),
            (Kind::Node, PathBuf::from("/usr/bin/node")),
            (Kind::Bun, PathBuf::from("/usr/bin/bun")),
        ];
        let ask = |kind: Kind, exe: &Path| -> Result<Option<String>, Error> {
            let v = match exe.to_str().unwrap() {
                "/old/deno" => "1.46.3",
                "/nvm/v20/node" => "20.18.1",
                "/usr/bin/node" => "24.11.0",
                _ => "1.3.2",
            };
            if old_enough(kind, v) {
                Ok(Some(v.to_owned()))
            } else {
                Err(Error::TooOld {
                    name: kind.label(),
                    version: v.to_owned(),
                    oldest: kind.oldest().unwrap(),
                })
            }
        };
        let got = first_usable(found.clone(), ask).unwrap();
        assert_eq!(got.exe, PathBuf::from("/usr/bin/node"));
        assert_eq!(got.version.as_deref(), Some("24.11.0"));
        assert_eq!(got.from, Origin::System);
        // None answers: none.
        assert_eq!(
            first_usable(found, |_, _| Err(Error::NotTheProgram("x"))),
            None
        );
    }

    /// A computer's Node.js past its line's end of life is passed over for MDM's own; one still
    /// supported, or of a line newer than this MDM knows, is used.
    #[test]
    fn node_past_its_end_of_life_is_passed_over() {
        assert_eq!(days(1970, 1, 1), 0);
        assert_eq!(days(2026, 11, 1), 20_758);
        let nov_2026 = days(2026, 11, 1);
        let v = |s: &str| Some(Some(s.to_owned()));
        assert_eq!(
            judge(Kind::Node, v("24.19.0"), nov_2026)
                .unwrap()
                .as_deref(),
            Some("24.19.0")
        );
        assert_eq!(
            judge(Kind::Node, v("22.23.5"), nov_2026)
                .unwrap()
                .as_deref(),
            Some("22.23.5")
        );
        assert!(judge(Kind::Node, v("22.23.5"), days(2027, 5, 1)).is_err());
        assert!(matches!(
            judge(Kind::Node, v("25.9.0"), nov_2026),
            Err(Error::TooOld {
                oldest: "26.0.0",
                ..
            })
        ));
        assert!(
            judge(Kind::Node, v("20.18.1"), nov_2026).is_err(),
            "too old for yt-dlp"
        );
        assert!(judge(Kind::Node, v("30.1.0"), nov_2026).is_ok());
        assert!(judge(Kind::Node, None, nov_2026).is_err(), "didn't answer");
        // Deno has no such schedule here.
        assert!(judge(Kind::Deno, v("2.5.6"), days(2035, 1, 1)).is_ok());
    }

    /// MDM's own copy counts only in a version yt-dlp can use.
    #[test]
    fn mdms_own_copy_too_old_is_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("node");
        let exe = root
            .join("20.18.1")
            .join(format!("node{}", std::env::consts::EXE_SUFFIX));
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"x").unwrap();
        crate::make_current(
            &root,
            Installed {
                version: "20.18.1".into(),
                exe,
            },
        )
        .unwrap();
        assert_eq!(own(dir.path()), None);
    }

    /// A Deno MDM installed that the person then chose as their runtime stays.
    #[test]
    fn a_chosen_deno_in_mdms_folder_is_not_removed() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let put = |tool: &str, version: &str, program: &str| {
            let root = d.join(tool);
            let exe = root
                .join(version)
                .join(format!("{program}{}", std::env::consts::EXE_SUFFIX));
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(&exe, b"x").unwrap();
            crate::make_current(
                &root,
                Installed {
                    version: version.into(),
                    exe,
                },
            )
            .unwrap()
        };
        let deno = put("deno", "2.5.0", "deno");
        put("node", "26.13.1", "node");
        crate::chosen::choose_runtime(
            d,
            crate::chosen::ChosenRuntime {
                kind: "deno".into(),
                version: "2.5.0".into(),
                exe: deno.exe.clone(),
            },
        )
        .unwrap();
        assert!(!retire_deno(d));
        assert!(deno.exe.is_file());
        forget(d).unwrap();
        assert!(retire_deno(d));
        assert!(!deno.exe.exists());
    }

    /// Told without running anything: a chosen runtime, or MDM's own when the computer has no
    /// runtime the folders show.
    #[test]
    fn the_runtime_is_told_without_asking_when_it_can() {
        let dir = tempfile::tempdir().unwrap();
        let before = probes();
        let mine = dir.path().join("mine-node");
        std::fs::write(&mine, b"x").unwrap();
        crate::chosen::choose_runtime(
            dir.path(),
            crate::chosen::ChosenRuntime {
                kind: "node".into(),
                version: "24.11.0".into(),
                exe: mine.clone(),
            },
        )
        .unwrap();
        let now = current_now(Some(dir.path()), None).unwrap().unwrap();
        assert_eq!((now.exe, now.from), (mine, Origin::Chosen));
        assert_eq!(probes(), before);
    }

    /// MDM's own: its Node.js, else the Deno an earlier version installed; once Node.js is in
    /// place that Deno is removed, and nothing else in the tools folder is.
    #[test]
    fn mdms_deno_gives_way_to_its_node() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(own(d), None);
        let put = |tool: &str, version: &str, program: &str| {
            let root = d.join(tool);
            let exe = root
                .join(version)
                .join(format!("{program}{}", std::env::consts::EXE_SUFFIX));
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(&exe, b"x").unwrap();
            crate::make_current(
                &root,
                Installed {
                    version: version.into(),
                    exe,
                },
            )
            .unwrap()
        };
        let deno = put("deno", "2.5.0", "deno");
        std::fs::create_dir_all(d.join("yt-dlp")).unwrap();
        assert_eq!(own(d).unwrap().kind, Kind::Deno);
        assert_eq!(own(d).unwrap().exe, deno.exe);
        // Without MDM's Node.js, its Deno stays.
        assert!(!retire_deno(d));
        assert!(deno.exe.is_file());
        let node = put("node", "26.13.1", "node");
        let now = own(d).unwrap();
        assert_eq!((now.kind, now.exe.clone()), (Kind::Node, node.exe.clone()));
        assert!(retire_deno(d));
        assert!(!d.join("deno").exists());
        assert!(node.exe.is_file());
        assert!(d.join("yt-dlp").is_dir());
        assert_eq!(own(d).unwrap().exe, node.exe);
    }
}
