//! Subtitles a site refuses never cost the video (#68), against a fake yt-dlp that answers as the
//! real one did for YouTube's automatic English captions: `Unable to download video subtitles for
//! 'en': HTTP Error 429: Too Many Requests`. Without `--ignore-errors` the fake fails the whole
//! run, as yt-dlp does; with it, it warns and saves the video.
//!
//! One program, three roles (no test harness, `harness = false`):
//! - run as `fake-ytdlp[.exe]` (a copy of itself): it is yt-dlp;
//! - run with `MDM_SUBS_RETRY_SECS` set (the retry's pause, shortened): it runs the checks;
//! - run plainly (`cargo test`): it runs itself again with that variable, the way to set it
//!   without touching its own environment.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use mdm_tools::video::{State, SubReason, Subs, VideoJob};

fn main() -> ExitCode {
    let me = std::env::current_exe().expect("own path");
    let name = me.file_stem().map(|s| s.to_string_lossy().into_owned());
    if name.as_deref() == Some("fake-ytdlp") {
        return fake_ytdlp();
    }
    if std::env::var_os("MDM_SUBS_RETRY_SECS").is_some() {
        checks(&me);
        job_checks(&me);
        list_checks(&me);
        println!("fake_ytdlp: ok");
        return ExitCode::SUCCESS;
    }
    let status = Command::new(&me)
        .env("MDM_SUBS_RETRY_SECS", "1")
        .status()
        .expect("run the checks");
    if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// What the fake does, from a file beside the output: `refuse-always` (English is refused every
/// time) or `refuse-once` (the first time only).
fn mode(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("mode.txt")).unwrap_or_default()
}

/// A process the fake leaves behind (as yt-dlp's launcher leaves its child, and ffmpeg): it
/// writes to `file` every 100 ms for 30 s, unless ended.
fn linger(file: &Path) -> ExitCode {
    use std::io::Write;
    let end = Instant::now() + Duration::from_secs(30);
    while Instant::now() < end {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
        {
            let _ = f.write_all(b"x");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    ExitCode::SUCCESS
}

/// Raw bytes on stdout (a line in another code page than UTF-8, #79).
fn out_bytes(b: &[u8]) {
    use std::io::Write;
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(b);
    let _ = o.flush();
}

fn fake_ytdlp() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--linger") {
        return linger(Path::new(&args[1]));
    }
    let after = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let has = |flag: &str| args.iter().any(|a| a == flag);
    // #69: `https://list.example/playlist/<n>` (its size said) or `.../channel/<n>` (not).
    if let Some(list) = args
        .last()
        .and_then(|u| u.strip_prefix("https://list.example/"))
    {
        return fake_list(list, has("-J"), after("--playlist-items"));
    }
    // #352: yt-dlp's answer when the cookies leave it nothing to offer.
    if args
        .last()
        .is_some_and(|u| u == "https://cookies.example/none")
        && has("--cookies")
    {
        eprintln!(
            "ERROR: [youtube] abc: Requested format is not available. Use --list-formats for a list of available formats"
        );
        return ExitCode::FAILURE;
    }
    if has("-J") {
        const PAGE: &str = r#"{"title": "Talk", "language": "ru", "formats": [
            {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a"}],
            "automatic_captions": {"en": [{"name": "English"}], "ru": [{"name": "Russian"}]}}"#;
        println!("{PAGE}");
        return ExitCode::SUCCESS;
    }
    let template = after("-o").expect("-o");
    let dir = Path::new(&template).parent().expect("a folder").to_owned();
    let mode = mode(&dir);
    let tries = dir.join("tries.txt");
    let tried = std::fs::read_to_string(&tries)
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        .unwrap_or(0);
    std::fs::write(&tries, (tried + 1).to_string()).unwrap();
    let format = after("-f").unwrap_or_default();
    // #94: the chosen height isn't offered (the reroute left it out); a fallback is.
    if mode == "unavailable" && format.contains("[height=") {
        eprintln!(
            "ERROR: [youtube] abc: Requested format is not available. Use --list-formats for a list of available formats"
        );
        return ExitCode::FAILURE;
    }
    // #94: a media server that resolved when looked up doesn't when downloading, once.
    if mode == "dns-once" && tried == 0 {
        eprintln!(
            "ERROR: [download] Got error: HTTPSConnection(host='rr1---sn-dead.googlevideo.com', port=443): Failed to resolve 'rr1---sn-dead.googlevideo.com' ([Errno 11004] getaddrinfo failed). Giving up after 10 retries"
        );
        return ExitCode::FAILURE;
    }
    println!("MDMP 1024 4096 NA 1000");
    println!("MDMQ 18 640x360");
    // #79: it falls over mid-way, leaving a helper running (Windows: the launcher's child).
    if mode == "crash" {
        let me = std::env::current_exe().unwrap();
        let _ = Command::new(me)
            .args(["--linger", &dir.join("linger.log").to_string_lossy()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        std::thread::sleep(Duration::from_millis(500));
        eprintln!("ERROR: unable to download video data: HTTP Error 500");
        return ExitCode::FAILURE;
    }
    // #79: lines that aren't UTF-8, before and as the final path (a legacy code page).
    if mode == "odd-lines" {
        out_bytes(b"\xff\xfe [download] Destination: \xde\xfd.mp4\n");
        println!("not one of our lines");
        println!("MDMP NA NA NA NA");
    }
    // #79: silent for a while, as when ffmpeg merges a long video.
    if let Some(secs) = mode.strip_prefix("quiet:") {
        std::thread::sleep(Duration::from_secs(secs.trim().parse().unwrap_or(1)));
    }
    let langs = after("--sub-langs").unwrap_or_default();
    for lang in langs.split(',').filter(|l| !l.is_empty()) {
        let refused = lang == "en" && (mode == "refuse-always" || tried == 0);
        if refused {
            let why =
                "Unable to download video subtitles for 'en': HTTP Error 429: Too Many Requests";
            if has("--ignore-errors") {
                eprintln!("WARNING: {why}");
                continue;
            }
            eprintln!("ERROR: {why}");
            return ExitCode::FAILURE;
        }
        let file = template.replace("%(ext)s", &format!("{lang}.srt"));
        std::fs::write(file, "1\n00:00:00,000 --> 00:00:01,000\nHello\n").unwrap();
    }
    if !has("--skip-download") {
        let video = template.replace("%(ext)s", "mp4").replace("%%", "%");
        std::fs::write(&video, vec![0u8; 4096]).unwrap();
        if mode == "odd-lines" {
            // The path in a one-byte code page: every letter beyond ASCII one byte, not UTF-8.
            let mut line = b"MDMF ".to_vec();
            line.extend(
                video
                    .chars()
                    .map(|c| if c.is_ascii() { c as u8 } else { 0xde }),
            );
            line.push(b'\n');
            out_bytes(&line);
        } else {
            println!("MDMF {video}");
        }
    }
    ExitCode::SUCCESS
}

/// A list of `n` videos as yt-dlp lists them: `-J` (flat, one entry) says what it is; flat
/// `-j` prints the entries in `--playlist-items`' range, a line each, a little at a time (as
/// YouTube's pages of 100 come). Every 7th is private.
fn fake_list(list: &str, whole: bool, items: Option<String>) -> ExitCode {
    let (kind, n) = list.split_once('/').unwrap_or((list, "0"));
    // `?fail=<i>`: the site stops answering at entry i (on a listing from the start only: listed
    // again from there, as after a pause, it goes on).
    let (n, fail) = match n.split_once("?fail=") {
        Some((n, f)) => (n, f.parse::<u64>().ok()),
        None => (n, None),
    };
    let n: u64 = n.parse().unwrap_or(0);
    let count = if kind == "playlist" {
        format!(r#", "playlist_count": {n}"#)
    } else {
        String::new()
    };
    if whole {
        println!(r#"{{"_type": "playlist", "title": "A fake {kind}"{count}, "entries": []}}"#);
        return ExitCode::SUCCESS;
    }
    let (from, to) = items
        .as_deref()
        .and_then(|r| r.split_once(':'))
        .map(|(a, b)| (a.parse().unwrap_or(1), b.parse().unwrap_or(n)))
        .unwrap_or((1, n));
    for i in from..=to.min(n) {
        if fail == Some(i) && from == 1 {
            eprintln!(
                "ERROR: [youtube:tab] Unable to download API page: HTTP Error 429: Too Many Requests"
            );
            return ExitCode::FAILURE;
        }
        let title = if i % 7 == 0 {
            "[Private video]".to_owned()
        } else {
            format!("Video {i}")
        };
        println!(
            r#"{{"_type": "url", "url": "https://video.example/v{i}", "title": "{title}", "duration": {}, "playlist_index": {i}, "playlist_title": "A fake {kind}"{count}}}"#,
            60 + i % 600
        );
        if i % 100 == 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    ExitCode::SUCCESS
}

/// The fake, as a copy of this program named `fake-ytdlp` in `dir`.
fn fake_in(me: &Path, dir: &Path) -> PathBuf {
    let exe = dir.join(format!("fake-ytdlp{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(me, &exe).expect("copy the fake");
    exe
}

/// Download with `subs` while the fake behaves as `mode`; the end state, its log and folder.
fn download(me: &Path, mode: &str, subs: &Subs) -> (mdm_tools::video::Progress, Vec<String>) {
    let (p, files, _out) = download_as(me, mode, subs, "b", "Talk");
    (p, files)
}

/// The same with a format `selector` and a file `stem`; also hands back the output folder.
fn download_as(
    me: &Path,
    mode: &str,
    subs: &Subs,
    selector: &str,
    stem: &str,
) -> (mdm_tools::video::Progress, Vec<String>, tempfile::TempDir) {
    let tools = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::fs::write(out.path().join("mode.txt"), mode).unwrap();
    let exe = fake_in(me, tools.path());
    let job = VideoJob::start(
        &exe,
        "https://video.example/talk",
        selector,
        out.path(),
        stem,
        None,
        1,
        None,
        None,
        &Default::default(),
        subs,
    )
    .expect("started");
    let end = Instant::now() + Duration::from_secs(120);
    let p = loop {
        let p = job.progress();
        if p.state != State::Running {
            break p;
        }
        assert!(Instant::now() < end, "still running: {p:?}");
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut files: Vec<String> = std::fs::read_dir(out.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.ends_with(".txt") && !n.ends_with(".log"))
        .collect();
    files.sort();
    (p, files, out)
}

fn log_of(p: &mdm_tools::video::Progress) -> Vec<String> {
    p.events.iter().map(|(_, t)| t.clone()).collect()
}

/// #79 and #94: the job never gives up while yt-dlp works, never leaves it running when it does
/// give up, and mends what a new look-up or a nearby quality can mend.
fn job_checks(me: &Path) {
    let none = Subs::default();

    // Silent for 30 s (ffmpeg merging a long video): no timeout of MDM's own ends it.
    let (p, files, _o) = download_as(me, "quiet:30", &none, "b", "Talk");
    assert!(matches!(p.state, State::Done(_)), "quiet: {:?}", p.state);
    assert_eq!(files, ["Talk.mp4"]);

    // Lines that aren't UTF-8, the final path among them: read through, the file found by name.
    let (p, files, _o) = download_as(me, "odd-lines", &none, "b", "Şirin ğüş");
    match &p.state {
        State::Done(f) => assert_eq!(f.file_name().unwrap().to_string_lossy(), "Şirin ğüş.mp4"),
        other => panic!("odd lines: {other:?}"),
    }
    assert_eq!(files, ["Şirin ğüş.mp4"]);

    // It falls over mid-way: the failure is told, and what it left running is ended with it
    // (Windows: the job; elsewhere yt-dlp is one process, so there's no child to leave).
    let (p, _files, out) = download_as(me, "crash", &none, "b", "Talk");
    match &p.state {
        State::Failed(e) => assert!(e.contains("HTTP Error 500"), "{e}"),
        other => panic!("crash: {other:?}"),
    }
    #[cfg(windows)]
    {
        let size = || std::fs::metadata(out.path().join("linger.log")).map_or(0, |m| m.len());
        std::thread::sleep(Duration::from_millis(800));
        let after = size();
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(
            size(),
            after,
            "the helper it left was ended with the download"
        );
    }
    #[cfg(not(windows))]
    let _ = out;

    // The chosen height isn't offered (a reroute left it out): the nearest below, said so.
    let (p, files, _o) = download_as(
        me,
        "unavailable",
        &none,
        "bv*[height=1080]+ba/b[height=1080]",
        "Talk",
    );
    assert!(
        matches!(p.state, State::Done(_)),
        "unavailable: {:?}",
        p.state
    );
    assert_eq!(files, ["Talk.mp4"]);
    let log = log_of(&p);
    assert!(
        log.iter()
            .any(|t| t.starts_with("The chosen quality isn't offered")),
        "{log:?}"
    );
    assert!(
        log.iter().any(|t| t == "Downloaded 18 640x360 instead"),
        "{log:?}"
    );

    // A media server that stopped resolving: looked up again without it, then saved.
    let (p, files, _o) = download_as(me, "dns-once", &none, "b", "Talk");
    assert!(matches!(p.state, State::Done(_)), "dns: {:?}", p.state);
    assert_eq!(files, ["Talk.mp4"]);
    let log = log_of(&p);
    assert!(
        log.iter().any(|t| t
            == "The server rr1---sn-dead.googlevideo.com couldn't be reached; looking the page up again without it"),
        "{log:?}"
    );
}

/// #69: a list is told from a video, listed whole as it comes (thousands, in order), a channel a
/// page at a time, and a listing stops when asked, ending yt-dlp.
fn list_checks(me: &Path) {
    use mdm_tools::playlist::{Listing, PAGE};
    use mdm_tools::resolve::{Looked, look_up};
    let tools = tempfile::tempdir().unwrap();
    let exe = fake_in(me, tools.path());
    let none = Default::default();
    let wait = |l: &Listing| {
        let end = Instant::now() + Duration::from_secs(60);
        loop {
            let p = l.progress();
            if p.done {
                return p;
            }
            assert!(Instant::now() < end, "still listing: {p:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    match look_up(&exe, "https://list.example/playlist/6000", &none).unwrap() {
        Looked::List(l) => assert_eq!((l.title.as_str(), l.total), ("A fake playlist", Some(6000))),
        Looked::Video(_) => panic!("a playlist taken for a video"),
    }
    assert!(matches!(
        look_up(&exe, "https://video.example/talk", &none).unwrap(),
        Looked::Video(_)
    ));

    // #352: cookies that leave yt-dlp nothing to offer: the same page without them.
    let page = "https://cookies.example/none";
    let cookies = mdm_tools::proc::FromBrowser {
        cookies: Some("LOGIN_INFO=l; SAPISID=s".into()),
        ..Default::default()
    };
    assert!(matches!(
        look_up(&exe, page, &cookies).unwrap(),
        Looked::Video(_)
    ));
    let (args, _file) = cookies.args(page).unwrap();
    let out =
        mdm_tools::proc::look_up(&exe, &["-J".to_owned()], page, Duration::from_secs(30)).unwrap();
    assert!(!out.without_cookies, "no cookies, so nothing to retry");
    let first = ["-J".to_owned()]
        .into_iter()
        .chain(args)
        .collect::<Vec<_>>();
    let out = mdm_tools::proc::look_up(&exe, &first, page, Duration::from_secs(30)).unwrap();
    assert!(out.without_cookies, "the second look-up, without them");

    // Six thousand, whole: all of them, in order, the private ones marked.
    let t = Instant::now();
    let l = Listing::start(&exe, "https://list.example/playlist/6000", &none, 1, None).unwrap();
    let p = wait(&l);
    let all = l.entries_from(0);
    assert_eq!((all.len(), p.total, p.error), (6000, Some(6000), None));
    assert!(all.iter().enumerate().all(|(i, e)| e.index == i as u64 + 1));
    assert!(all[6].unavailable && !all[7].unavailable);
    assert_eq!(l.entries_from(5990).len(), 10);
    println!("fake_ytdlp: 6000 listed in {:?}", t.elapsed());

    // A channel (no size said): a page, then the next from where it stopped, then the end.
    let url = "https://list.example/channel/120";
    let l = Listing::start(&exe, url, &none, 1, Some(PAGE)).unwrap();
    let p = wait(&l);
    assert_eq!((l.entries_from(0).len(), p.total), (PAGE as usize, None));
    let l = Listing::start(&exe, url, &none, 101, Some(150)).unwrap();
    wait(&l);
    let last = l.entries_from(0);
    assert_eq!((last.len(), last[0].index), (20, 101));

    // The site stops answering part-way: what was listed stays, and why it stopped is told;
    // listed again from the next one, it goes on to the end.
    let url = "https://list.example/playlist/300?fail=121";
    let l = Listing::start(&exe, url, &none, 1, None).unwrap();
    let p = wait(&l);
    assert_eq!(l.entries_from(0).len(), 120);
    assert!(p.error.as_deref().unwrap_or("").contains("429"), "{p:?}");
    let l = Listing::start(&exe, url, &none, 121, None).unwrap();
    let p = wait(&l);
    let rest = l.entries_from(0);
    assert_eq!((rest.len(), rest[0].index, p.error), (180, 121, None));

    // Stopped part-way: what was listed stays, and it's done at once.
    let l = Listing::start(&exe, "https://list.example/playlist/20000", &none, 1, None).unwrap();
    while l.progress().listed < 150 {
        std::thread::sleep(Duration::from_millis(10));
    }
    l.cancel();
    let p = l.progress();
    let kept = l.entries_from(0).len();
    assert!(p.done && (150..20000).contains(&kept), "{p:?}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        l.entries_from(0).len(),
        kept,
        "nothing listed after the stop"
    );
}

fn checks(me: &Path) {
    let subs = |langs: &[&str], embed: bool| Subs {
        langs: langs.iter().map(|l| l.to_string()).collect(),
        automatic: true,
        embed,
    };

    // English refused every time: the video and the Russian ones are saved, English is told.
    let (p, files) = download(me, "refuse-always", &subs(&["en", "ru"], false));
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(files, ["Talk.mp4", "Talk.ru.srt"]);
    assert_eq!(p.sub_problems.len(), 1, "{:?}", p.sub_problems);
    let problem = &p.sub_problems[0];
    assert_eq!(
        (
            problem.code.as_str(),
            problem.name.as_str(),
            &problem.reason,
            problem.beside
        ),
        ("en", "English", &SubReason::TooManyRequests, false)
    );
    let log: Vec<&str> = p.events.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        log.iter()
            .any(|t| t.starts_with("Subtitles in en couldn't be downloaded")),
        "{log:?}"
    );

    // Refused once only: the second try, after the pause, brings them.
    let (p, files) = download(me, "refuse-once", &subs(&["en"], false));
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(files, ["Talk.en.srt", "Talk.mp4"]);
    assert!(p.sub_problems.is_empty(), "{:?}", p.sub_problems);
    assert_eq!(
        p.side_files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        ["Talk.en.srt"]
    );

    // Asked into the video (no ffmpeg here, so beside it): still never the video's failure.
    let (p, files) = download(me, "refuse-always", &subs(&["en"], true));
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(files, ["Talk.mp4"]);
    assert_eq!(p.sub_problems.len(), 1);
}
