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
//!
//! #769: with `page.json` beside it, the fake's page is that one (its formats' addresses on a lab
//! server, `{LINK}` replaced by a name of its own each look-up); asked for its choice
//! (`-O "MDMJ %()j"`) it answers as yt-dlp does after choosing; and asked to download a choice
//! whose streams are already there (MDM's engine downloaded them), it merges them as yt-dlp
//! does ("has already been downloaded"), downloading only what isn't there.
//!
//! #686: with `page-signed-in.json` beside it too, a look-up with the browser's cookies gets that
//! page (a signed-in viewer's, whose streams the lab refuses); it notes every run's cookies in
//! `cookies.log`, and refuses two YouTube addresses as YouTube does (`fixtures/ytdlp-refusals`).

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
    if name.as_deref() == Some("fake-ffmpeg") {
        return fake_ffmpeg();
    }
    if name.as_deref() == Some("fake-ffprobe") {
        return fake_ffprobe();
    }
    if std::env::var_os("MDM_SUBS_RETRY_SECS").is_some() {
        private_cookie_checks(&me);
        proxy_checks(&me);
        held_output_checks(&me);
        checks(&me);
        job_checks(&me);
        merge_checks(&me);
        list_checks(&me);
        preloaded_checks(&me);
        engine_checks(&me);
        recording_checks(&me);
        long_name_checks(&me);
        saved_checks(&me);
        live_checks(&me);
        part_checks(&me);
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
/// writes to `file` every 100 ms for `secs` (30 s unless said), unless ended.
fn linger(file: &Path, secs: Option<&str>) -> ExitCode {
    use std::io::Write;
    let secs = secs.and_then(|s| s.parse().ok()).unwrap_or(30);
    let end = Instant::now() + Duration::from_secs(secs);
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

/// #1521: a process the fake leaves behind holding its stdout and stderr open for 90 s
/// ([`linger`], writing to `file`), as an ffmpeg yt-dlp started and shares its output with can.
fn holder(file: &Path) {
    let me = std::env::current_exe().unwrap();
    let _ = Command::new(me)
        .args(["--linger", &file.to_string_lossy(), "90"])
        .stdin(std::process::Stdio::null())
        .spawn();
}

/// Raw bytes on stdout (a line in another code page than UTF-8, #79).
fn out_bytes(b: &[u8]) {
    use std::io::Write;
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(b);
    let _ = o.flush();
}

/// The page the fake's look-up (`-J`) gives.
const PAGE: &str = r#"{"title": "Talk", "language": "ru", "formats": [
    {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a"}],
    "automatic_captions": {"en": [{"name": "English"}], "ru": [{"name": "Russian"}]}}"#;

fn fake_ytdlp() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--linger") {
        return linger(Path::new(&args[1]), args.get(2).map(String::as_str));
    }
    // `--version` as yt-dlp answers it, and nothing else: MDM asks a yt-dlp it didn't install
    // whether it has `--js-runtimes` (2025.11.12, #1448) before telling it of this computer's
    // JavaScript runtime. Not a run of its own (not in the logs below).
    if args == ["--version"] {
        println!("2026.10.26");
        return ExitCode::SUCCESS;
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
    let home = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_owned))
        .expect("the fake's folder");
    if home.join("record-cookie-args").exists() {
        use std::io::Write;
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.join("cookie-args.jsonl"))
            .unwrap();
        writeln!(log, "{}", serde_json::to_string(&args).unwrap()).unwrap();
    }
    // #686: every run beside the fake, in order, with the browser's cookies or without them:
    // `J` a look-up, `O` a choice of formats, `D` a download.
    {
        use std::io::Write;
        let kind = if has("-J") {
            "J"
        } else if after("-O").is_some_and(|o| o.starts_with("MDMJ")) {
            "O"
        } else {
            "D"
        };
        let cookies = if has("--cookies") { "with" } else { "without" };
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(home.join("cookies.log"))
        {
            let _ = writeln!(f, "{kind} {cookies}");
        }
    }
    // #686: YouTube's bot check refusing a signed-in look-up, and a channel members' video,
    // refused with the cookies and without them; in yt-dlp's real words.
    let video = args.last().map(String::as_str).unwrap_or_default();
    if has("-J") && video.ends_with("v=botcheck000") && has("--cookies") {
        eprint!(
            "{}",
            include_str!("fixtures/ytdlp-refusals/bot-check.stderr")
        );
        return ExitCode::FAILURE;
    }
    if has("-J") && video.ends_with("v=members0000") {
        eprint!("{}", include_str!("fixtures/ytdlp-refusals/members.stderr"));
        return ExitCode::FAILURE;
    }
    if has("-J") {
        // Each look-up counted beside the fake: the quality menu's is taken instead (A2).
        let file = home.join("looked.txt");
        let n = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| t.trim().parse::<u32>().ok())
            .unwrap_or(0);
        let _ = std::fs::write(&file, (n + 1).to_string());
        // Every look-up after the first slow (3 s), when asked: a moment to act in between runs.
        if n >= 1 && home.join("slow-second-look-up").exists() {
            std::thread::sleep(Duration::from_secs(3));
        }
        // #1521: a process left behind holding the output open, when asked.
        if home.join("hold-output").exists() {
            holder(&home.join("linger.log"));
        }
        // #852: YouTube looked up without a JavaScript runtime, as yt-dlp says it.
        if args
            .last()
            .is_some_and(|u| u == "https://nojs.example/watch")
        {
            eprint!(
                "{}",
                include_str!("fixtures/ytdlp-js-runtime/no-runtime.stderr")
            );
        }
        // #769: a page of the test's, its links signed anew (`link=` a name of this look-up's own,
        // which the lab times from its first request); #686: the signed-in viewer's
        // (`page-signed-in.json`) when the look-up has the browser's cookies.
        let signed_in = home.join("page-signed-in.json");
        let page = if has("--cookies") && signed_in.is_file() {
            signed_in
        } else {
            home.join("page.json")
        };
        match std::fs::read_to_string(page) {
            Ok(page) => {
                let ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis();
                let link = format!("{}-{ms}", std::process::id());
                println!("{}", page.replace("{LINK}", &link));
            }
            Err(_) => println!("{PAGE}"),
        }
        return ExitCode::SUCCESS;
    }
    // #769: the choice (`-O "MDMJ %()j"`), as yt-dlp prints it after choosing the formats.
    if after("-O").is_some_and(|o| o.starts_with("MDMJ")) {
        let info =
            std::fs::read_to_string(after("--load-info-json").expect("a page")).expect("the page");
        let info: serde_json::Value = serde_json::from_str(&info).expect("json");
        let template = after("-o").expect("-o").replace("%%", "%");
        let selector = after("-f").unwrap_or_default();
        // #94, #1240: the chosen height isn't offered, for the engine's choice too.
        let dir = Path::new(&template).parent().expect("a folder");
        if mode(dir) == "unavailable" && selector.contains("[height=") {
            eprintln!(
                "ERROR: [youtube] talk: Requested format is not available. Use --list-formats for a list of available formats"
            );
            return ExitCode::FAILURE;
        }
        let chosen = choice(&info, &selector, &template);
        // #1655: streams served only from a few seconds after the choice, as YouTube's after the
        // ad it plays ahead of the video (yt-dlp's `available_at`): `"available_at": "soon"` and
        // `from=SOON` in their addresses become that time, counted from now, however long the
        // look-up took.
        let soon = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 5;
        let chosen = chosen
            .to_string()
            .replace(
                "\"available_at\":\"soon\"",
                &format!("\"available_at\":{soon}"),
            )
            .replace("from=SOON", &format!("from={}", soon * 1000));
        println!("MDMJ {chosen}");
        return ExitCode::SUCCESS;
    }
    let template = after("-o").expect("-o");
    let dir = Path::new(&template).parent().expect("a folder").to_owned();
    let mode = mode(&dir);
    // The page it was given to download from, kept for the checks to read.
    if let Some(info) = after("--load-info-json") {
        let _ = std::fs::copy(info, dir.join("loaded.txt"));
    }
    let tries = dir.join("tries.txt");
    let tried = std::fs::read_to_string(&tries)
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        .unwrap_or(0);
    std::fs::write(&tries, (tried + 1).to_string()).unwrap();
    let format = after("-f").unwrap_or_default();
    // #1662: a part, as yt-dlp's ffmpeg downloads one.
    if let Some(sections) = after("--download-sections") {
        return fake_part(&args, &sections, &dir, &template, &format, &mode);
    }
    // A live stream, recorded as yt-dlp's ffmpeg records one: into `<stem>.mp4.part` (written
    // over, `-y`), MPEG-TS (`live`; `live-raw`: not), growing, saying nothing of its progress,
    // never ending by itself (a minute at most). `live-dns-once`: its first run fails a while
    // in, a media server no longer found. `live-ends`: the stream ends 3 s in, and yt-dlp
    // finishes the file itself. `live-split`: the picture and the sound recorded apart.
    // `live-late`: as ffmpeg opens its file late and writes its first block later still: after
    // `MDMQ` it waits (30 s at most) for `open.txt` in the folder, creates the `.part` empty, then
    // waits (30 s at most) for `go.txt` before it writes as `live` does.
    if mode.starts_with("live") {
        use std::io::Write;
        println!("MDMQ 96 1280x720");
        let part = format!(
            "{}.part",
            template.replace("%(ext)s", "mp4").replace("%%", "%")
        );
        let mut packet = [0u8; 188];
        if mode != "live-raw" {
            packet[0] = 0x47;
        }
        let chunk = packet.repeat(348);
        // `live-split`: the picture and the sound apart, each a stream's own part (the sound's
        // a third as big).
        if mode == "live-split" {
            let stream = |id: &str, ext: &str| {
                format!(
                    "{}.part",
                    template
                        .replace("%(ext)s", &format!("f{id}.{ext}"))
                        .replace("%%", "%")
                )
            };
            let mut video = std::fs::File::create(stream("1", "mp4")).unwrap();
            let mut audio = std::fs::File::create(stream("2", "m4a")).unwrap();
            let end = Instant::now() + Duration::from_secs(60);
            while Instant::now() < end {
                video.write_all(&chunk).unwrap();
                audio.write_all(&chunk[..116 * 188]).unwrap();
                std::thread::sleep(Duration::from_millis(100));
            }
            eprintln!("ERROR: the fake live stream went on too long");
            return ExitCode::FAILURE;
        }
        // `live-late`: a file that isn't there until the test says (`open.txt`) and holds nothing
        // until it says again (`go.txt`).
        let wait_for = |name: &str| {
            let until = Instant::now() + Duration::from_secs(30);
            while !dir.join(name).exists() {
                if Instant::now() >= until {
                    eprintln!("ERROR: the fake live stream waited too long for {name}");
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            true
        };
        if mode == "live-late" && !wait_for("open.txt") {
            return ExitCode::FAILURE;
        }
        let mut file = std::fs::File::create(&part).unwrap();
        if mode == "live-late" && !wait_for("go.txt") {
            return ExitCode::FAILURE;
        }
        let end = Instant::now() + Duration::from_secs(60);
        let mut written = 0;
        while Instant::now() < end {
            file.write_all(&chunk).unwrap();
            file.flush().unwrap();
            written += chunk.len();
            if mode == "live-dns-once" && tried == 0 && written >= 256 << 10 {
                eprintln!(
                    "ERROR: [download] Got error: HTTPSConnection(host='rr1---sn-dead.googlevideo.com', port=443): Failed to resolve 'rr1---sn-dead.googlevideo.com' ([Errno 11004] getaddrinfo failed). Giving up after 10 retries"
                );
                return ExitCode::FAILURE;
            }
            // The stream ends (3 s in): as yt-dlp says it once its ffmpeg is done, all of it
            // in, then the file named and fixed up (`FixupM3u8`: an MP4 now), and said.
            if mode == "live-ends" && written >= 30 * chunk.len() {
                drop(file);
                println!("MDMP {written} {written} NA 650000 96");
                let video = template.replace("%(ext)s", "mp4").replace("%%", "%");
                std::fs::rename(&part, &video).unwrap();
                if args.iter().any(|a| a.starts_with("post_process:MDMM")) {
                    println!("MDMM {video}");
                }
                let mut fixed = b"FAKEMP4 ".to_vec();
                fixed.extend(std::fs::read(&video).unwrap());
                std::fs::write(&video, fixed).unwrap();
                println!("MDMF {video}");
                return ExitCode::SUCCESS;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        eprintln!("ERROR: the fake live stream went on too long");
        return ExitCode::FAILURE;
    }
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
    // #736: a video and its sound, merged as yt-dlp's ffmpeg does: into `<stem>.temp.mp4`,
    // growing for a while, then renamed. Said first when asked (`post_process:` print). Halfway,
    // the file is empty for a while (written again from the start): what is said of the merge
    // must not go back with it. Whole, it stays a while before the rename, long enough to be
    // read on a slow machine.
    if mode == "merge" {
        use std::io::Write;
        println!("MDMQ 137+140 1920x1080");
        println!("MDMP 3000 3000 NA 1000 137");
        println!("MDMP 1000 1000 NA 1000 140");
        let video = template.replace("%(ext)s", "mp4").replace("%%", "%");
        if args.iter().any(|a| a.starts_with("post_process:MDMM")) {
            println!("MDMM {video}");
        }
        let temp = video.replace(".mp4", ".temp.mp4");
        let mut file = std::fs::File::create(&temp).unwrap();
        for k in 1..=10u64 {
            if k == 6 {
                file.set_len(0).unwrap();
                std::thread::sleep(Duration::from_millis(600));
                file = std::fs::File::create(&temp).unwrap();
                file.write_all(&[0u8; 2000]).unwrap();
            }
            file.write_all(&[0u8; 400]).unwrap();
            file.flush().unwrap();
            std::thread::sleep(Duration::from_millis(300));
        }
        drop(file);
        std::thread::sleep(Duration::from_millis(1200));
        std::fs::rename(&temp, &video).unwrap();
        println!("MDMF {video}");
        return ExitCode::SUCCESS;
    }
    // A merge that fails (a stream ffmpeg can't read): the streams stay, whole, as yt-dlp leaves
    // them, and the merge's file half written.
    if mode == "merge-fails" {
        println!("MDMQ 137+aud-English 1920x1080");
        println!("MDMP 3000 3000 NA 1000 137");
        println!("MDMP 1000 1000 NA 1000 aud-English");
        let video = template.replace("%(ext)s", "mp4").replace("%%", "%");
        std::fs::write(video.replace(".mp4", ".f137.mp4"), [0u8; 3000]).unwrap();
        // An HLS sound, named as yt-dlp names its rendition.
        std::fs::write(video.replace(".mp4", ".faud-English.mp4"), [0u8; 1000]).unwrap();
        if args.iter().any(|a| a.starts_with("post_process:MDMM")) {
            println!("MDMM {video}");
        }
        std::fs::write(video.replace(".mp4", ".temp.mp4"), [0u8; 1200]).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        eprintln!("ERROR: Postprocessing: Conversion failed!");
        return ExitCode::FAILURE;
    }
    println!("MDMP 1024 4096 NA 1000 18");
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
    // #1521: it ends (well, or falling over) leaving behind a process that still holds its
    // output open, as an ffmpeg yt-dlp started and shares its output with can.
    if mode == "held" || mode == "held-crash" {
        holder(&dir.join("linger.log"));
        if mode == "held-crash" {
            eprintln!("ERROR: unable to download video data: HTTP Error 500");
            return ExitCode::FAILURE;
        }
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
    // #569: the format asked for, for the checks.
    std::fs::write(dir.join("format.txt"), &format).unwrap();
    // #769: a choice whose streams MDM's engine downloaded: merged as yt-dlp merges them.
    if let Some(info) = after("--load-info-json")
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter(|i| i["formats"][0]["url"].is_string())
        && !has("--skip-download")
    {
        let template = template.replace("%%", "%");
        let chosen = choice(&info, &format, &template);
        let output = chosen["filename"].as_str().unwrap().to_owned();
        let streams: Vec<String> = match chosen["requested_formats"].as_array() {
            Some(fs) => fs
                .iter()
                .map(|f| {
                    output.replace(
                        ".mp4",
                        &format!(
                            ".f{}.{}",
                            f["format_id"].as_str().unwrap(),
                            f["ext"].as_str().unwrap()
                        ),
                    )
                })
                .collect(),
            None => vec![output.clone()],
        };
        let mut merged = Vec::new();
        for s in &streams {
            match std::fs::read(s) {
                // "has already been downloaded"
                Ok(b) => merged.extend(b),
                // Downloaded here, through yt-dlp's own unfinished files (#1102): `.part`, and
                // the fragments' resume file, `.ytdl`.
                Err(_) => {
                    for temp in [format!("{s}.part"), format!("{s}.ytdl")] {
                        if let Err(failed) = python_write(&temp, b"") {
                            return failed;
                        }
                    }
                    let _ = std::fs::remove_file(format!("{s}.part"));
                    let _ = std::fs::remove_file(format!("{s}.ytdl"));
                    merged.extend(b"downloaded by yt-dlp");
                }
            }
        }
        for s in streams.iter().filter(|s| **s != output) {
            let _ = std::fs::remove_file(s);
        }
        // #736: several streams are merged as yt-dlp's ffmpeg does, said first when asked
        // (`post_process:` print), into `<stem>.temp.mp4`, growing for a while, then renamed.
        if streams.len() > 1 && args.iter().any(|a| a.starts_with("post_process:MDMM")) {
            println!("MDMQ {} 1920x1080", chosen["format_id"].as_str().unwrap());
            println!("MDMM {output}");
            let temp = output.replace(".mp4", ".temp.mp4");
            let step = merged.len().div_ceil(5);
            for k in 1..=5 {
                if let Err(failed) = python_write(&temp, &merged[..(k * step).min(merged.len())]) {
                    return failed;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
            std::fs::rename(&temp, &output).unwrap();
        } else if let Err(failed) = python_write(&output, &merged) {
            return failed;
        }
        println!("MDMF {output}");
        return ExitCode::SUCCESS;
    }
    if !has("--skip-download") {
        let video = template.replace("%(ext)s", "mp4").replace("%%", "%");
        // #569: YouTube's HLS sound joined without ffmpeg, as yt-dlp leaves it: pieces each led
        // by an ID3 tag, then raw AAC (ADTS) frames, under the format's `.mp4` name.
        let content = if mode == "hls-audio" {
            let mut piece = b"ID3     ".to_vec();
            piece.extend([0u8; 16]);
            piece.extend([0xff, 0xf1, 0x50, 0x80, 0x2e, 0x7f, 0xfc]);
            piece.repeat(64)
        } else {
            vec![0u8; 4096]
        };
        std::fs::write(&video, content).unwrap();
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

/// #1662: a part (`--download-sections "*<from>-<to>"`), downloaded as yt-dlp's ffmpeg downloads
/// one: each chosen stream read from its address (MDM's server's, with the engine; the lab's
/// otherwise) from where the part starts, by its share of the stream (the page's `duration`),
/// over one open-ended request as ffmpeg asks (`Range: bytes=<start>-`), until it has the part's
/// share; joined into `<output>.part`, then the file, said. `part-slow` (mode): a little at a
/// time. Kept for the checks beside the output: its arguments (`part-args.txt`, a line each),
/// what each stream's server answered (`part-status.txt`), the addresses it read
/// (`part-urls.txt`) and its `no_proxy` (`part-env.txt`).
fn fake_part(
    args: &[String],
    sections: &str,
    dir: &Path,
    template: &str,
    format: &str,
    mode: &str,
) -> ExitCode {
    use std::io::{Read, Write};
    std::fs::write(dir.join("part-args.txt"), args.join("\n")).unwrap();
    std::fs::write(
        dir.join("part-env.txt"),
        std::env::var("no_proxy").unwrap_or_default(),
    )
    .unwrap();
    let (from, to) = sections
        .trim_start_matches('*')
        .split_once('-')
        .map(|(a, b)| (a.parse::<f64>().unwrap(), b.parse::<f64>().unwrap()))
        .expect("*<from>-<to>");
    let at = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let info: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(at("--load-info-json").expect("a page")).unwrap(),
    )
    .unwrap();
    let chosen = choice(&info, format, &template.replace("%%", "%"));
    let duration = info["duration"].as_f64().unwrap_or(100.0);
    println!(
        "MDMQ {} 1280x720",
        chosen["format_id"].as_str().unwrap_or("NA")
    );
    let output = chosen["filename"].as_str().unwrap().to_owned();
    let part = format!("{output}.part");
    let mut file = std::fs::File::create(&part).unwrap();
    let formats = chosen["requested_formats"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![chosen.clone()]);
    let (mut statuses, mut urls) = (Vec::new(), Vec::new());
    for f in formats {
        let url = url::Url::parse(f["url"].as_str().unwrap()).unwrap();
        urls.push(url.to_string());
        let size = f["filesize"].as_u64().unwrap();
        let start = (size as f64 * from / duration) as u64;
        let len = (size as f64 * (to - from) / duration) as u64;
        let mut s =
            std::net::TcpStream::connect((url.host_str().unwrap(), url.port().unwrap())).unwrap();
        let path = match url.query() {
            Some(q) => format!("{}?{q}", url.path()),
            None => url.path().to_owned(),
        };
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Lavf/61.7.100\r\nRange: bytes={start}-\r\nConnection: close\r\n\r\n",
            url.host_str().unwrap()
        )
        .unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if s.read(&mut byte).unwrap() == 0 {
                break;
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        statuses.push(head.lines().next().unwrap_or("").to_owned());
        let mut left = len;
        let mut buf = vec![0u8; 64 * 1024];
        while left > 0 {
            let want = buf.len().min(left as usize);
            let n = s.read(&mut buf[..want]).unwrap_or(0);
            if n == 0 {
                eprintln!("ERROR: [ffmpeg] the stream ended early");
                return ExitCode::FAILURE;
            }
            file.write_all(&buf[..n]).unwrap();
            left -= n as u64;
            if mode == "part-slow" {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    std::fs::write(dir.join("part-status.txt"), statuses.join("\n")).unwrap();
    std::fs::write(dir.join("part-urls.txt"), urls.join("\n")).unwrap();
    drop(file);
    std::fs::rename(&part, &output).unwrap();
    println!("MDMF {output}");
    ExitCode::SUCCESS
}

/// #1662: a part of a video, its streams read through MDM's server (`partserve`) at MDM's pace,
/// only from where the part starts, in requests of a few MB; without the engine yt-dlp's ffmpeg
/// reads them itself. A quick cut asks yt-dlp for nothing more; an exact one for its keyframes
/// at the cuts and the encoders the ffmpeg has. No subtitles, no chapters. Stopped, it starts
/// again and ends whole; under a speed limit it takes as long as the limit makes it.
fn part_checks(me: &Path) {
    use mdm_labserver::{Config, LabServer, content};
    use mdm_tools::part::Part;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let lab = rt.block_on(LabServer::start(Config::default())).unwrap();
    let engine = |limit: Option<u64>| mdm_tools::streams::Engine {
        handle: rt.handle().clone(),
        opts: mdm_engine::Options {
            max_connections: 4,
            retry_delay: Duration::from_millis(50),
            speed_limit: limit,
            ..Default::default()
        },
    };
    const V: u64 = 4 << 20;
    const A: u64 = 1 << 20;
    let page = format!(
        r#"{{"id": "talk", "title": "Talk", "ext": "mp4", "duration": 100, "formats": [
        {{"format_id": "137", "ext": "mp4", "protocol": "https", "filesize": {V},
          "vcodec": "avc1", "acodec": "none", "vbr": 1500, "url": "{}",
          "http_headers": {{"User-Agent": "FakeTube/1"}}}},
        {{"format_id": "140", "ext": "m4a", "protocol": "https", "filesize": {A},
          "vcodec": "none", "acodec": "mp4a", "abr": 128, "url": "{}"}}]}}"#,
        lab.url("/gen/4M/v.mp4?seed=1"),
        lab.url("/gen/1M/a.m4a?seed=2")
    );
    // The part's bytes of each stream, as the fake reads them: from 30 s to 60 s of 100.
    let expected = || {
        let share = |size: u64, seed: u64| {
            let (start, len) = (size * 30 / 100, size * 30 / 100);
            let mut v = vec![0; len as usize];
            content::fill(seed, start, &mut v);
            v
        };
        let mut v = share(V, 1);
        v.extend(share(A, 2));
        v
    };
    let part = Part {
        from: 30,
        to: 60,
        exact: false,
    };
    let start = |tools: &Path,
                 out: &Path,
                 engine: Option<mdm_tools::streams::Engine>,
                 part: Part,
                 ffmpeg: Option<&Path>| {
        // Started again in the same folder: the fake there already (a stopped one's program may
        // still be held a moment by the system).
        let own = tools.join(format!("fake-ytdlp{}", std::env::consts::EXE_SUFFIX));
        let exe = if own.is_file() {
            own
        } else {
            fake_in(me, tools)
        };
        std::fs::write(tools.join("page.json"), &page).unwrap();
        VideoJob::start_with(
            &exe,
            "https://video.example/talk",
            "bv*+ba/b",
            out,
            "Talk",
            None,
            ffmpeg,
            None,
            &Default::default(),
            &Subs {
                langs: vec!["en".into()],
                automatic: false,
                embed: false,
            },
            None,
            engine,
            None,
            Some(part),
        )
        .expect("started")
    };
    let wait = |job: &VideoJob| {
        let end = Instant::now() + Duration::from_secs(120);
        let mut seen = Vec::new();
        loop {
            let p = job.progress();
            if p.state == State::Running && p.downloaded > 0 {
                seen.push((p.downloaded, p.total));
            }
            if p.state != State::Running {
                return (p, seen);
            }
            assert!(Instant::now() < end, "still running: {:?}", log_of(&p));
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let read = |dir: &Path, name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();

    // A quick cut through MDM's server.
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let job = start(tools.path(), out.path(), Some(engine(None)), part, None);
    let (p, seen) = wait(&job);
    let log = log_of(&p);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert!(
        p.helper.is_none(),
        "MDM's connections, not yt-dlp's own: {:?}",
        p.helper
    );
    let saved = out.path().join("Talk.mp4");
    assert!(
        std::fs::read(&saved).unwrap() == expected(),
        "the part's bytes, in order"
    );
    let args = read(out.path(), "part-args.txt");
    let args: Vec<&str> = args.lines().collect();
    let at = args
        .iter()
        .position(|a| *a == "--download-sections")
        .expect("sections");
    assert_eq!(args[at + 1], "*30-60");
    for gone in [
        "--force-keyframes-at-cuts",
        "--embed-chapters",
        "--sub-langs",
        "--write-subs",
        "--fixup",
    ] {
        assert!(!args.contains(&gone), "{gone} in {args:?}");
    }
    let urls = read(out.path(), "part-urls.txt");
    let lab_at = lab.url("/");
    assert!(
        urls.lines().count() == 2
            && urls
                .lines()
                .all(|u| u.starts_with("http://127.0.0.1:") && !u.starts_with(lab_at.as_str())),
        "read through MDM's server: {urls}"
    );
    assert!(
        read(out.path(), "part-status.txt")
            .lines()
            .all(|s| s.starts_with("HTTP/1.1 206")),
        "{}",
        read(out.path(), "part-status.txt")
    );
    assert!(read(out.path(), "part-env.txt").contains("127.0.0.1"));
    assert!(
        log.iter()
            .any(|l| l == "Only the part from 0:30 to 1:00: a quick cut, at the keyframes"),
        "{log:?}"
    );
    assert!(
        log.iter()
            .any(|l| l == "ffmpeg reads the part through MDM's connections"),
        "{log:?}"
    );
    // Its progress: the bytes brought in, against the part's share of the streams (1.5 MB).
    assert!(!seen.is_empty(), "progress while it ran");
    assert!(
        seen.iter().all(|(_, t)| t.is_some_and(|t| t >= 1_500_000)),
        "{seen:?}"
    );
    rt.block_on(lab.settled());
    let asked: Vec<(u64, u64)> = lab
        .log()
        .into_iter()
        .filter(|e| e.path.starts_with("/gen/4M/v.mp4"))
        .filter_map(|e| {
            let r = e.range?;
            let (a, b) = r.trim_start_matches("bytes=").split_once('-')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .collect();
    assert!(!asked.is_empty());
    // Nothing before the part, nothing open-ended, a few MB at most each, and far from the
    // whole stream in all.
    assert!(
        asked
            .iter()
            .all(|(a, b)| *a >= V * 30 / 100 && b - a < 2 << 20),
        "{asked:?}"
    );
    let total: u64 = asked.iter().map(|(a, b)| b - a + 1).sum();
    assert!(total < V * 3 / 4, "{total} of {V}");
    assert!(
        lab.log()
            .iter()
            .any(|e| e.user_agent.as_deref() == Some("FakeTube/1")),
        "yt-dlp's headers for the stream go with MDM's requests"
    );

    // An exact cut: keyframes at the cuts, and the encoders the ffmpeg has (MDM's own build's).
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let ffmpeg = tools
        .path()
        .join(format!("fake-ffmpeg{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(me, &ffmpeg).unwrap();
    let job = start(
        tools.path(),
        out.path(),
        Some(engine(None)),
        Part {
            exact: true,
            ..part
        },
        Some(&ffmpeg),
    );
    let (p, _) = wait(&job);
    assert!(
        matches!(p.state, State::Done(_)),
        "{:?}: {:?}",
        p.state,
        log_of(&p)
    );
    let args = read(out.path(), "part-args.txt");
    let args: Vec<&str> = args.lines().collect();
    assert!(args.contains(&"--force-keyframes-at-cuts"), "{args:?}");
    let at = args
        .iter()
        .position(|a| *a == "--downloader-args")
        .expect("the encoders");
    assert_eq!(
        args[at + 1],
        "ffmpeg_o:-c:v libopenh264 -pix_fmt yuv420p -b:v 3000k -maxrate 6000k -bufsize 6000k -c:a aac -b:a 192k"
    );
    assert!(
        log_of(&p)
            .iter()
            .any(|l| l.ends_with("an exact cut, encoded again"))
    );

    // Stopped half-way, started again: the part comes whole (ffmpeg can't go on from where it
    // was; the part is read again).
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(out.path().join("mode.txt"), "part-slow").unwrap();
    let job = start(tools.path(), out.path(), Some(engine(None)), part, None);
    let end = Instant::now() + Duration::from_secs(60);
    while job.progress().downloaded < 300_000 {
        assert!(
            Instant::now() < end,
            "no progress: {:?}",
            log_of(&job.progress())
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    job.stop();
    let end = Instant::now() + Duration::from_secs(30);
    while job.progress().state == State::Running {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(job.progress().state, State::Stopped);
    assert!(!out.path().join("Talk.mp4").exists());
    let job = start(tools.path(), out.path(), Some(engine(None)), part, None);
    let (p, _) = wait(&job);
    assert!(
        matches!(p.state, State::Done(_)),
        "{:?}: {:?}",
        p.state,
        log_of(&p)
    );
    assert!(std::fs::read(out.path().join("Talk.mp4")).unwrap() == expected());

    // Under a speed limit: 1.5 MB at 400 KB/s takes about 4 s (a second's burst allowed).
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let began = Instant::now();
    let job = start(
        tools.path(),
        out.path(),
        Some(engine(Some(400_000))),
        part,
        None,
    );
    let (p, _) = wait(&job);
    assert!(
        matches!(p.state, State::Done(_)),
        "{:?}: {:?}",
        p.state,
        log_of(&p)
    );
    let took = began.elapsed();
    assert!(
        took >= Duration::from_millis(2000),
        "the limit held: {took:?}"
    );

    // Without MDM's engine, yt-dlp's ffmpeg reads the streams itself: the same part.
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let job = start(tools.path(), out.path(), None, part, None);
    let (p, _) = wait(&job);
    assert!(
        matches!(p.state, State::Done(_)),
        "{:?}: {:?}",
        p.state,
        log_of(&p)
    );
    assert!(std::fs::read(out.path().join("Talk.mp4")).unwrap() == expected());
    assert!(
        read(out.path(), "part-urls.txt")
            .lines()
            .all(|u| u.starts_with(lab_at.as_str())),
        "{}",
        read(out.path(), "part-urls.txt")
    );
    assert_eq!(read(out.path(), "part-env.txt"), "");
}

/// Write `path` as yt-dlp's Python does on a new install (#1102): on Windows, with long paths
/// off (the default), a path past 259 UTF-16 units fails as `[Errno 2] No such file or
/// directory`; a name past 255 (UTF-16 units on Windows, bytes elsewhere) as `[Errno 22]
/// Invalid argument` on Windows, `[Errno 36] File name too long` elsewhere. Said as yt-dlp says
/// it, the path in Python's quotes.
fn python_write(path: &str, data: &[u8]) -> Result<(), ExitCode> {
    let units = |s: &str| {
        if cfg!(windows) {
            s.encode_utf16().count()
        } else {
            s.len()
        }
    };
    let long_name = Path::new(path)
        .iter()
        .any(|n| units(&n.to_string_lossy()) > 255);
    let error = if cfg!(windows) && units(path) > 259 {
        Some("[Errno 2] No such file or directory")
    } else if long_name && cfg!(windows) {
        Some("[Errno 22] Invalid argument")
    } else if long_name {
        Some("[Errno 36] File name too long")
    } else {
        None
    };
    if let Some(error) = error {
        let escaped = path.replace('\\', "\\\\");
        let quoted = if path.contains('\'') && !path.contains('"') {
            format!("\"{escaped}\"")
        } else {
            format!("'{}'", escaped.replace('\'', "\\'"))
        };
        eprintln!("ERROR: Unable to download video: {error}: {quoted}");
        return Err(ExitCode::FAILURE);
    }
    std::fs::write(path, data).unwrap();
    Ok(())
}

/// yt-dlp's choice of formats on page `info` for `selector`: a format id the page has, that one;
/// else every format of the page merged (two or more), or the only one. With `filename` from the
/// output `template`.
fn choice(info: &serde_json::Value, selector: &str, template: &str) -> serde_json::Value {
    let formats = info["formats"].as_array().cloned().unwrap_or_default();
    let mut chosen = info.clone();
    let one = formats
        .iter()
        .find(|f| f["format_id"].as_str() == Some(selector))
        .or((formats.len() == 1).then(|| &formats[0]));
    match one {
        Some(f) => {
            for (k, v) in f.as_object().unwrap() {
                chosen[k] = v.clone();
            }
        }
        None => {
            chosen["requested_formats"] = serde_json::Value::Array(formats.clone());
            chosen["ext"] = "mp4".into();
            chosen["format_id"] = formats
                .iter()
                .map(|f| f["format_id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
                .join("+")
                .into();
        }
    }
    let ext = chosen["ext"].as_str().unwrap_or("mp4").to_owned();
    chosen["filename"] = template.replace("%(ext)s", &ext).into();
    chosen
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

/// What the fake MP4s start with (not MPEG-TS): not counted as what they play.
const MARK: &[u8] = b"FAKEMP4 ";

/// How fast the fake recordings play: what ffprobe (the fake) says of their length.
const BYTES_PER_SECOND: f64 = 100_000.0;

/// MDM's ffprobe asked how long a file plays (its last argument, `file:<path>`), as JSON: its
/// bytes (after [`MARK`]) at [`BYTES_PER_SECOND`], the file's and each stream's (an MP4: a picture
/// and a sound; a recording's part: one). `ffprobe-mode.txt` beside it: `short` (an MP4 plays
/// half as long), `short-sound` (an MP4's sound does), `estimate` (said estimated from the
/// bitrate) or `fail` (it can't tell).
fn fake_ffprobe() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let last = args.last().expect("a file");
    let path = PathBuf::from(last.strip_prefix("file:").unwrap_or(last));
    let mode = std::fs::read_to_string(path.with_file_name("ffprobe-mode.txt")).unwrap_or_default();
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("{}: No such file or directory", path.display());
        return ExitCode::FAILURE;
    };
    if mode.trim() == "fail" {
        eprintln!(
            "{}: Invalid data found when processing input",
            path.display()
        );
        return ExitCode::FAILURE;
    }
    if mode.trim() == "estimate" {
        eprintln!("[mpegts @ 0000] Estimating duration from bitrate, this may be inaccurate");
    }
    let plays = bytes.strip_prefix(MARK).unwrap_or(&bytes).len() as f64 / BYTES_PER_SECOND;
    let mp4 = path.extension().is_some_and(|e| e == "mp4");
    let half = |short: bool| if short && mp4 { plays / 2.0 } else { plays };
    let (file, sound) = (half(mode.trim() == "short"), half(mode.trim() != ""));
    let stream =
        |kind: &str, d: f64| format!(r#"{{"codec_type": "{kind}", "duration": "{d:.6}"}}"#);
    let mut streams = vec![stream("video", file)];
    if mp4 {
        let sound = if mode.trim() == "short-sound" {
            sound
        } else {
            file
        };
        streams.push(stream("audio", sound));
    }
    println!(
        r#"{{"streams": [{}], "format": {{"duration": "{file:.6}"}}}}"#,
        streams.join(", ")
    );
    ExitCode::SUCCESS
}

/// MDM's ffmpeg making a live stream's recording an MP4 (`-i file:<.part> [-i file:<.part>…] …
/// file:<out>`): the recordings copied one after the other after [`MARK`], taking a second. Its
/// runs counted in `ffmpeg-runs.txt` beside the (first) recording; `ffmpeg-mode.txt` there:
/// `fail` (half written, then an error), `tiny` (a few bytes, no error), `short` (four fifths, no
/// error) or `linger` (its file there 2 s before it ends).
fn fake_ffmpeg() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // #1662: its encoders, as MDM's own Windows build (LGPL: no x264) lists them.
    if args.iter().any(|a| a == "-encoders") {
        println!(
            "Encoders:\n V..... = Video\n A..... = Audio\n ------\n V....D libopenh264          OpenH264 H.264\n V.S..D mpeg4                MPEG-4 part 2\n V....D libvpx-vp9           libvpx VP9\n A....D aac                  AAC (Advanced Audio Coding)\n A....D libopus              libopus Opus"
        );
        return ExitCode::SUCCESS;
    }
    let file = |a: &str| PathBuf::from(a.strip_prefix("file:").unwrap_or(a));
    let inputs: Vec<PathBuf> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "-i")
        .filter_map(|(i, _)| args.get(i + 1))
        .map(|a| file(a))
        .collect();
    let input = inputs.first().expect("-i").clone();
    let output = file(args.last().expect("an output"));
    let dir = input.parent().expect("a folder").to_owned();
    let runs = dir.join("ffmpeg-runs.txt");
    let n = std::fs::read_to_string(&runs)
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        .unwrap_or(0);
    std::fs::write(&runs, (n + 1).to_string()).unwrap();
    let mode = std::fs::read_to_string(dir.join("ffmpeg-mode.txt")).unwrap_or_default();
    let recorded: Vec<u8> = inputs
        .iter()
        .flat_map(|i| std::fs::read(i).expect("the recording"))
        .collect();
    std::thread::sleep(Duration::from_secs(1));
    match mode.trim() {
        "fail" => {
            std::fs::write(&output, &recorded[..recorded.len() / 2]).unwrap();
            eprintln!("[mp4 @ 0000] Could not write header: Invalid argument");
            ExitCode::FAILURE
        }
        "tiny" => {
            std::fs::write(&output, MARK).unwrap();
            ExitCode::SUCCESS
        }
        "short" => {
            let mut made = MARK.to_vec();
            made.extend(&recorded[..recorded.len() * 4 / 5]);
            std::fs::write(&output, made).unwrap();
            ExitCode::SUCCESS
        }
        mode => {
            let mut made = MARK.to_vec();
            made.extend(recorded);
            std::fs::write(&output, made).unwrap();
            // Its file there a while before it ends (`linger`): long enough for another program
            // to open it.
            if mode == "linger" {
                std::thread::sleep(Duration::from_secs(2));
            }
            ExitCode::SUCCESS
        }
    }
}

/// The fake, as a copy of this program named `fake-ytdlp` in `dir`.
fn fake_in(me: &Path, dir: &Path) -> PathBuf {
    let exe = dir.join(format!("fake-ytdlp{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(me, &exe).expect("copy the fake");
    exe
}

/// Options' browser store is on: inspect arguments received by an actual child process for
/// look-ups and downloads, both private (empty, own header, own jar) and ordinary requests.
fn private_cookie_checks(me: &Path) {
    use mdm_tools::proc::{BrowserCookies, FromBrowser, set_browser_cookies};
    set_browser_cookies(BrowserCookies::new("firefox", "normal-profile"));
    let private = FromBrowser {
        private: true,
        ..Default::default()
    };
    let header = FromBrowser {
        cookies: Some("sid=private".into()),
        ..private.clone()
    };
    let jar = FromBrowser {
        jar: vec![mdm_ipc::BrowserCookie {
            name: "sid".into(),
            value: "private".into(),
            domain: "video.example".into(),
            path: "/".into(),
            ..Default::default()
        }],
        ..private.clone()
    };
    for browser in [private.clone(), header, jar, FromBrowser::default()] {
        let tools = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let exe = fake_in(me, tools.path());
        std::fs::write(tools.path().join("record-cookie-args"), "").unwrap();
        let url = "https://video.example/talk";
        mdm_tools::resolve::look_up(&exe, url, &browser).unwrap();
        let job = VideoJob::start(
            &exe,
            url,
            "b",
            out.path(),
            "Talk",
            None,
            None,
            None,
            &browser,
            &Subs::default(),
            None,
        )
        .unwrap();
        let until = Instant::now() + Duration::from_secs(30);
        let progress = loop {
            let progress = job.progress();
            if progress.state != State::Running {
                break progress;
            }
            assert!(Instant::now() < until, "still running: {progress:?}");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(matches!(progress.state, State::Done(_)), "{progress:?}");
        let runs: Vec<Vec<String>> =
            std::fs::read_to_string(tools.path().join("cookie-args.jsonl"))
                .unwrap()
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect();
        assert!(
            runs.iter().any(|args| args.iter().any(|a| a == "-J")),
            "look-up ran"
        );
        assert!(
            runs.iter()
                .any(|args| args.iter().any(|a| a == "--load-info-json")),
            "download ran"
        );
        let own = browser.cookies.is_some() || !browser.jar.is_empty();
        for args in runs {
            assert_eq!(
                args.iter().any(|a| a == "--cookies-from-browser"),
                !browser.private,
                "{args:?}"
            );
            assert_eq!(args.iter().any(|a| a == "--cookies"), own, "{args:?}");
            assert_eq!(
                args.iter().any(|a| a == "--no-cookies-from-browser"),
                browser.private && !own,
                "{args:?}"
            );
        }
        assert_eq!(
            progress
                .events
                .iter()
                .any(|(_, e)| e.contains("Cookies read from firefox")),
            !browser.private
        );
    }
    // A private members-only video fails once with the original authentication refusal;
    // retrying must never borrow the normal profile to make it succeed.
    let tools = tempfile::tempdir().unwrap();
    let exe = fake_in(me, tools.path());
    std::fs::write(tools.path().join("record-cookie-args"), "").unwrap();
    let error = mdm_tools::resolve::look_up(
        &exe,
        "https://www.youtube.com/watch?v=members0000",
        &private,
    )
    .unwrap_err();
    assert!(error.to_string().contains("members-only"), "{error}");
    let log = std::fs::read_to_string(tools.path().join("cookie-args.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 1, "no cookie retry: {log}");
    assert!(!log.contains("\"--cookies-from-browser\""), "{log}");
    set_browser_cookies(None);
}

/// #1448: Options' automatic configuration script reaches every run of yt-dlp for a video, the
/// look-up and the download from its looked-up page alike: each is asked about the page. The
/// download's run went straight to the site, the script asked about no address.
fn proxy_checks(me: &Path) {
    let tools = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let exe = fake_in(me, tools.path());
    std::fs::write(tools.path().join("record-cookie-args"), "").unwrap();
    mdm_tools::proc::set_proxy(mdm_engine::Proxy::Pac {
        script: std::sync::Arc::from(
            "function FindProxyForURL(u,h){return h=='video.example'?'PROXY 127.0.0.1:3128':'DIRECT';}",
        ),
    });
    let job = VideoJob::start(
        &exe,
        "https://video.example/talk",
        "b",
        out.path(),
        "Talk",
        None,
        None,
        None,
        &Default::default(),
        &Subs::default(),
        None,
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(60);
    let progress = loop {
        let progress = job.progress();
        if progress.state != State::Running {
            break progress;
        }
        assert!(Instant::now() < until, "still running: {progress:?}");
        std::thread::sleep(Duration::from_millis(20));
    };
    mdm_tools::proc::set_proxy(mdm_engine::Proxy::None);
    assert!(matches!(progress.state, State::Done(_)), "{progress:?}");
    let runs: Vec<Vec<String>> = std::fs::read_to_string(tools.path().join("cookie-args.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert!(
        runs.iter()
            .any(|args| args.iter().any(|a| a == "--load-info-json")),
        "download ran: {runs:?}"
    );
    for args in &runs {
        let at = args.iter().position(|a| a == "--proxy");
        assert_eq!(
            at.and_then(|i| args.get(i + 1)).map(String::as_str),
            Some("http://127.0.0.1:3128"),
            "{args:?}"
        );
    }
}

/// #1521: yt-dlp ended, but a process it started still holds its output open (an ffmpeg that
/// shares it; here for 90 s): the look-up and the download end with yt-dlp, not with that
/// process, and it is ended with them.
fn held_output_checks(me: &Path) {
    // Far below the holder's 90 s, far above the 2 s the output is read after yt-dlp's end.
    let soon = Duration::from_secs(30);
    // The holder writes to its file every 100 ms until it is ended.
    let ended = |file: &Path, what: &str| {
        let size = || std::fs::metadata(file).map_or(0, |m| m.len());
        std::thread::sleep(Duration::from_millis(800));
        let after = size();
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(size(), after, "{what}: what yt-dlp left was ended with it");
    };
    // A look-up: yt-dlp's `-J` output read to its end.
    let tools = tempfile::tempdir().unwrap();
    let exe = fake_in(me, tools.path());
    std::fs::write(tools.path().join("hold-output"), "").unwrap();
    let started = Instant::now();
    let looked =
        mdm_tools::resolve::look_up(&exe, "https://video.example/talk", &Default::default());
    let took = started.elapsed();
    assert!(looked.is_ok(), "look-up: {:?}", looked.err());
    assert!(
        took < soon,
        "the look-up waited {took:?} for what yt-dlp left"
    );
    ended(&tools.path().join("linger.log"), "look-up");
    // A download that ends well, and one that falls over.
    for mode in ["held", "held-crash"] {
        let started = Instant::now();
        let (p, files, out) = download_as(me, mode, &Subs::default(), "b", "Talk");
        let took = started.elapsed();
        assert!(
            took < soon,
            "{mode}: the download waited {took:?} for what yt-dlp left"
        );
        match &p.state {
            State::Done(_) if mode == "held" => assert_eq!(files, ["Talk.mp4"]),
            State::Failed(e) if mode == "held-crash" => {
                assert!(e.contains("HTTP Error 500"), "{e}");
            }
            other => panic!("{mode}: {other:?}"),
        }
        ended(&out.path().join("linger.log"), mode);
    }
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
        None,
        None,
        &Default::default(),
        subs,
        None,
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

/// #1103: the saved video is shown to `saved` (the app's mark that it came from the internet) on
/// the job's own thread before the job says it is done, and a disk slow to take the mark keeps
/// only that thread waiting: the progress, which the window reads every frame, answers at once.
fn saved_checks(me: &Path) {
    use std::sync::{Arc, Mutex};
    let tools = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::fs::write(out.path().join("mode.txt"), "").unwrap();
    let exe = fake_in(me, tools.path());
    // Each file shown, and when the hook returned; it takes 2 s, as a busy disk did (#1103: 8 s).
    let shown: Arc<Mutex<Vec<(PathBuf, Instant)>>> = Arc::default();
    let hook = {
        let shown = shown.clone();
        mdm_engine::FileHook::new(move |p: &Path| {
            std::thread::sleep(Duration::from_secs(2));
            shown.lock().unwrap().push((p.to_owned(), Instant::now()));
        })
    };
    let job = VideoJob::start_with(
        &exe,
        "https://video.example/talk",
        "b",
        out.path(),
        "Talk",
        None,
        None,
        None,
        &Default::default(),
        &Subs::default(),
        None,
        None,
        Some(hook),
        None,
    )
    .expect("started");
    let end = Instant::now() + Duration::from_secs(120);
    let mut slowest = Duration::ZERO;
    let (p, done_at) = loop {
        let asked = Instant::now();
        let p = job.progress();
        slowest = slowest.max(asked.elapsed());
        if p.state != State::Running {
            break (p, Instant::now());
        }
        assert!(Instant::now() < end, "still running: {p:?}");
        std::thread::sleep(Duration::from_millis(20));
    };
    let State::Done(file) = &p.state else {
        panic!("not done: {:?}", p.state)
    };
    let shown = shown.lock().unwrap().clone();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(&shown[0].0, file, "the saved file is the one marked");
    assert!(
        shown[0].1 <= done_at,
        "marked before it was said to be done"
    );
    assert!(
        slowest < Duration::from_millis(500),
        "the progress waited {slowest:?} for the mark"
    );
    println!("saved_checks: marked before done; the slowest look at the progress took {slowest:?}");
}

fn log_of(p: &mdm_tools::video::Progress) -> Vec<String> {
    p.events.iter().map(|(_, t)| t.clone()).collect()
}

/// #736: while yt-dlp's ffmpeg joins a video and its sound, the job says so, and how far it is
/// from the joined file's size; it says no more once the file is there.
fn merge_checks(me: &Path) {
    let tools = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::fs::write(out.path().join("mode.txt"), "merge").unwrap();
    let exe = fake_in(me, tools.path());
    let job = VideoJob::start(
        &exe,
        "https://video.example/talk",
        "bv*+ba",
        out.path(),
        "Talk",
        None,
        None,
        None,
        &Default::default(),
        &Subs::default(),
        None,
    )
    .expect("started");
    let end = Instant::now() + Duration::from_secs(60);
    let mut seen = Vec::new();
    let p = loop {
        let p = job.progress();
        if p.state != State::Running {
            break p;
        }
        if let Some(m) = p.merging {
            seen.push(m);
        }
        assert!(Instant::now() < end, "still running: {p:?}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(matches!(p.state, State::Done(_)), "merge: {:?}", p.state);
    assert_eq!(p.merging, None, "done: no longer merging");
    let known: Vec<f64> = seen.iter().filter_map(|m| *m).collect();
    assert!(!seen.is_empty(), "merging was said");
    assert!(
        known.iter().any(|f| *f > 0.0 && *f < 1.0),
        "a share done was said: {seen:?}"
    );
    // Never back, though the file was empty a while halfway; and on to its whole size, 4000 bytes
    // of the streams' 4000, held short of the end until the rename (99 %).
    assert!(
        known.windows(2).all(|w| w[0] <= w[1]),
        "it only goes forward: {known:?}"
    );
    assert_eq!(known.last(), Some(&0.99), "it reaches the end: {known:?}");

    // Joining them fails: the video fails at once (no other look-up or server mends it), says
    // why, keeps the streams for Resume and leaves nothing half joined.
    let (p, files, out) = download_as(me, "merge-fails", &Subs::default(), "bv*+ba", "Talk");
    match &p.state {
        State::Failed(e) => assert!(e.contains("Conversion failed"), "{e}"),
        other => panic!("merge fails: {other:?}"),
    }
    assert_eq!(p.merging, None);
    let kept = ["Talk.f137.mp4", "Talk.faud-English.mp4"];
    assert_eq!(files, kept, "the streams stay, the merge's file goes");
    let named: Vec<String> = p
        .merge_failed
        .expect("said to be the merge")
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(named, kept);
    assert_eq!(
        std::fs::read_to_string(out.path().join("tries.txt")).unwrap(),
        "1",
        "one run"
    );
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

    // #569: no ffmpeg (none given here), the sound alone: yt-dlp is asked for one that comes as
    // a file first, and what it joined from HLS pieces is saved for what it is, raw AAC.
    let (p, files, out) = download_as(me, "hls-audio", &none, "ba", "Talk");
    match &p.state {
        State::Done(f) => assert_eq!(f.file_name().unwrap().to_string_lossy(), "Talk.aac"),
        other => panic!("hls audio: {other:?}"),
    }
    assert_eq!(files, ["Talk.aac"]);
    assert_eq!(
        std::fs::read_to_string(out.path().join("format.txt")).unwrap(),
        "ba[protocol!*=m3u8]/ba"
    );
    // A file that is what it's named: as it is.
    let (p, files, _o) = download_as(me, "", &none, "ba", "Talk");
    assert!(matches!(p.state, State::Done(_)), "plain: {:?}", p.state);
    assert_eq!(files, ["Talk.mp4"]);

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

/// A2: the download chosen in the browser's quality menu takes the menu's look-up (the page yt-dlp
/// read for it) in its first run, without asking yt-dlp to read the page again; with the note
/// when that look-up had to leave the cookies out. A look-up that doesn't offer the choice made,
/// a failed one, and every run after the first read the page afresh.
fn preloaded_checks(me: &Path) {
    use mdm_tools::video::{InfoSlot, Preloaded};
    let run = |mode: &str, preloaded: Option<Preloaded>| {
        let tools = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::fs::write(out.path().join("mode.txt"), mode).unwrap();
        let exe = fake_in(me, tools.path());
        let job = VideoJob::start_with(
            &exe,
            "https://video.example/talk",
            "18/b[height=360][vcodec^=avc1]/b[height=360]",
            out.path(),
            "Talk",
            None,
            None,
            None,
            &Default::default(),
            &Subs::default(),
            preloaded,
            None,
            None,
            None,
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
        let looked = std::fs::read_to_string(tools.path().join("looked.txt"))
            .ok()
            .and_then(|t| t.trim().parse::<u32>().ok())
            .unwrap_or(0);
        let loaded = std::fs::read_to_string(out.path().join("loaded.txt")).unwrap_or_default();
        (p, looked, loaded)
    };
    let menu_of = |page: &str, label: &str, without_cookies: Option<bool>| {
        let slot = InfoSlot::new(Instant::now());
        match without_cookies {
            Some(w) => slot.fill(page.as_bytes().to_vec(), w),
            None => slot.fail(),
        }
        Some(Preloaded {
            slot,
            selector: "18/b[height=360][vcodec^=avc1]/b[height=360]".into(),
            label: label.into(),
            merge: false,
        })
    };
    let menu = |label: &str, without_cookies: Option<bool>| menu_of(PAGE, label, without_cookies);
    let note = "The site refused the look-up with the browser's cookies, or offered no format with them; looked the page up without them";

    let (p, looked, _) = run("", menu("360p MP4 · H.264", Some(true)));
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(looked, 0, "the menu's look-up, not another");
    assert!(log_of(&p).iter().any(|t| t == note), "{:?}", log_of(&p));

    // Its own look-up when there's none, as before.
    let (p, looked, _) = run("", None);
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(looked, 1);

    // The menu's look-up doesn't offer that choice under that label, or failed: afresh.
    for menu in [
        menu("720p MP4 · H.264", Some(false)),
        menu("360p MP4 · H.264", None),
    ] {
        let (p, looked, _) = run("", menu);
        assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
        assert_eq!(looked, 1);
    }

    // The first run fails (a server stopped resolving): the second reads the page afresh.
    let (p, looked, _) = run("dns-once", menu("360p MP4 · H.264", Some(false)));
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(looked, 1, "the retry's");

    // The menu's look-up names a media server this computer can't reach (#94): its links are
    // moved to the alternative the site lists, as a fresh look-up's would be, and yt-dlp
    // downloads from that list without reading the page again. The servers are the test's
    // (`check_servers_with`): none is ever asked.
    mdm_tools::video::check_servers_with(
        |host| host.contains("sn-alive"),
        |link| link.contains("sn-alive"),
    );
    let dead = "https://rr1---sn-dead.googlevideo.com/videoplayback?id=1&mn=sn-dead%2Csn-alive";
    let page = PAGE.replace(
        r#""acodec": "mp4a"}"#,
        &format!(r#""acodec": "mp4a", "protocol": "https", "url": "{dead}"}}"#),
    );
    assert!(page.contains(dead));
    let (p, looked, loaded) = run("", menu_of(&page, "360p MP4 · H.264", Some(false)));
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(looked, 0, "the menu's look-up, moved");
    assert!(
        loaded.contains("rr1---sn-alive.googlevideo.com") && !loaded.contains("rr1---sn-dead."),
        "{loaded}"
    );
    assert!(
        log_of(&p).iter().any(|t| t
            == "The server rr1---sn-dead.googlevideo.com can't be reached; downloading from rr1---sn-alive.googlevideo.com"),
        "{:?}",
        log_of(&p)
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
    // #852: yt-dlp's warnings reach the look-up. A video looked up without a JavaScript runtime
    // says so (yt-dlp's own words, `fixtures/ytdlp-js-runtime`); another doesn't.
    let runtime_said = |url: &str| match look_up(&exe, url, &none).unwrap() {
        Looked::Video(v) => v.needs_js_runtime,
        Looked::List(_) => panic!("a video taken for a list"),
    };
    assert!(!runtime_said("https://video.example/talk"));
    assert!(runtime_said("https://nojs.example/watch"));

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

/// #769: MDM's engine downloads what yt-dlp chose, from a lab server: each stream over several
/// connections, within yt-dlp's `http_chunk_size`, into the files yt-dlp then merges; a stopped
/// one goes on from where it stopped; an address that expires is asked of yt-dlp again; a new
/// speed limit applies at once; and a stream in pieces stays yt-dlp's to download, said so.
fn engine_checks(me: &Path) {
    use mdm_labserver::{Config, LabServer, content};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let lab = rt.block_on(LabServer::start(Config::default())).unwrap();
    let engine = |connections: u32| mdm_tools::streams::Engine {
        handle: rt.handle().clone(),
        opts: mdm_engine::Options {
            max_connections: connections,
            retry_delay: Duration::from_millis(50),
            ..Default::default()
        },
    };
    const V: u64 = 3 << 20;
    const A: u64 = 1 << 20;
    let page = |video: &str, audio: &str, protocol: &str| {
        format!(
            r#"{{"id": "talk", "title": "Talk", "ext": "mp4", "formats": [
            {{"format_id": "137", "ext": "mp4", "protocol": "{protocol}", "filesize": {V},
              "vcodec": "avc1", "acodec": "none", "url": "{}",
              "http_headers": {{"User-Agent": "FakeTube/1"}},
              "downloader_options": {{"http_chunk_size": 524288}}}},
            {{"format_id": "140", "ext": "m4a", "protocol": "https", "filesize": {A},
              "vcodec": "none", "acodec": "mp4a", "url": "{}"}}]}}"#,
            lab.url(video),
            lab.url(audio)
        )
    };
    let expected = || {
        let mut v = vec![0; V as usize];
        content::fill(1, 0, &mut v);
        let mut a = vec![0; A as usize];
        content::fill(2, 0, &mut a);
        v.extend(a);
        v
    };
    let start = |tools: &Path, out: &Path, connections: u32| {
        let exe = fake_in(me, tools);
        VideoJob::start(
            &exe,
            "https://video.example/talk",
            "bv*+ba/b",
            out,
            "Talk",
            None,
            None,
            None,
            &Default::default(),
            &Subs::default(),
            Some(engine(connections)),
        )
        .expect("started")
    };
    // What each run said of its merge (#736), in order.
    let merges = std::cell::RefCell::new(Vec::new());
    let wait = |job: &VideoJob| {
        let end = Instant::now() + Duration::from_secs(120);
        let mut most_connections = 0;
        merges.borrow_mut().clear();
        loop {
            let p = job.progress();
            if let Some(e) = &p.engine {
                most_connections = most_connections.max(e.connections.len());
            }
            if let Some(m) = p.merging {
                merges.borrow_mut().push(m);
            }
            if p.state != State::Running {
                return (p, most_connections);
            }
            assert!(Instant::now() < end, "still running: {:?}", log_of(&p));
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    // Merged: two streams, each over several connections, in requests of 512 KB at most.
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(
        tools.path().join("page.json"),
        page(
            "/gen/3M/v.mp4?seed=1&rate=8M",
            "/gen/1M/a.m4a?seed=2",
            "https",
        ),
    )
    .unwrap();
    let job = start(tools.path(), out.path(), 4);
    let (p, most) = wait(&job);
    let log = log_of(&p);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert!(p.helper.is_none());
    let saved = out.path().join("Talk.mp4");
    assert!(
        std::fs::read(&saved).unwrap() == expected(),
        "the streams merged"
    );
    assert!(most >= 2, "several connections: {most}");
    // #736: the engine downloaded the streams, yt-dlp's ffmpeg merges them: said, with how far.
    let known: Vec<f64> = merges.borrow().iter().filter_map(|m| *m).collect();
    assert!(
        known.iter().any(|f| *f > 0.0 && *f < 1.0),
        "the merge and its share done were said: {:?}",
        merges.borrow()
    );
    assert!(known.windows(2).all(|w| w[0] <= w[1]), "{known:?}");
    assert_eq!(p.merging, None, "done: no longer merging");
    assert!(
        log.iter()
            .any(|t| t == "yt-dlp's address for format 137: from 127.0.0.1"),
        "{log:?}"
    );
    rt.block_on(lab.settled());
    let video: Vec<_> = lab
        .log()
        .into_iter()
        .filter(|e| e.path.starts_with("/gen/3M/v.mp4"))
        .collect();
    assert!(video.iter().all(|e| {
        e.user_agent.as_deref() == Some("FakeTube/1")
            && e.range.as_deref().is_some_and(|r| {
                let (a, b) = r.trim_start_matches("bytes=").split_once('-').unwrap();
                b.parse::<u64>().unwrap() - a.parse::<u64>().unwrap() < 524288
            })
    }));
    assert!(
        std::fs::read_dir(out.path())
            .unwrap()
            .filter_map(Result::ok)
            .all(|e| !e.file_name().to_string_lossy().contains(".mdm.")),
        "no part file or state left"
    );

    // #1655: YouTube's streams are refused (403) until the ad played ahead of the video would
    // end, yt-dlp's `available_at`: the engine waits for it before its first request, says so,
    // and the download comes down (it failed at once before). The fake's choice makes "soon"
    // 5 s after it.
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut early: serde_json::Value = serde_json::from_str(&page(
        "/gen/3M/v.mp4?seed=1&from=SOON",
        "/gen/1M/a.m4a?seed=2&from=SOON",
        "https",
    ))
    .unwrap();
    for f in early["formats"].as_array_mut().unwrap() {
        f["available_at"] = "soon".into();
    }
    std::fs::write(tools.path().join("page.json"), early.to_string()).unwrap();
    let job = start(tools.path(), out.path(), 4);
    let (p, _) = wait(&job);
    let log = log_of(&p);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert!(
        std::fs::read(out.path().join("Talk.mp4")).unwrap() == expected(),
        "the streams merged"
    );
    assert!(
        log.iter()
            .any(|t| t.starts_with("Waiting ") && t.contains("as the site requires")),
        "the wait said: {log:?}"
    );
    rt.block_on(lab.settled());
    let streams: Vec<_> = lab
        .log()
        .into_iter()
        .filter(|e| e.query.as_deref().is_some_and(|q| q.contains("from=")))
        .collect();
    assert!(!streams.is_empty());
    // The lab answers 403 before `from`: none means none was asked for before its time.
    assert!(
        streams.iter().all(|e| e.status != 403),
        "a stream asked for before its time: {:?}",
        streams.iter().map(|e| e.status).collect::<Vec<_>>()
    );

    // #1240: the chosen height isn't offered (a reroute left it out): the engine downloads the
    // nearest below too, not the one asked for again.
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(
        tools.path().join("page.json"),
        page("/gen/3M/v.mp4?seed=1", "/gen/1M/a.m4a?seed=2", "https"),
    )
    .unwrap();
    std::fs::write(out.path().join("mode.txt"), "unavailable").unwrap();
    let job = VideoJob::start(
        &fake_in(me, tools.path()),
        "https://video.example/talk",
        "bv*[height=1080]+ba/b[height=1080]",
        out.path(),
        "Talk",
        None,
        None,
        None,
        &Default::default(),
        &Subs::default(),
        Some(engine(2)),
    )
    .expect("started");
    let (p, _) = wait(&job);
    let log = log_of(&p);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert!(p.helper.is_none(), "the engine downloaded it: {log:?}");
    assert!(
        log.iter()
            .any(|t| t.starts_with("The chosen quality isn't offered")),
        "{log:?}"
    );
    assert!(
        std::fs::read(out.path().join("Talk.mp4")).unwrap() == expected(),
        "the streams merged"
    );

    // Stopped half-way, then started again: it goes on from the bytes already there.
    lab.clear_log();
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(
        tools.path().join("page.json"),
        page(
            "/gen/3M/v.mp4?seed=1&rate=1M",
            "/gen/1M/a.m4a?seed=2",
            "https",
        ),
    )
    .unwrap();
    let job = start(tools.path(), out.path(), 2);
    while job.progress().downloaded < 1 << 20 {
        assert!(job.progress().state == State::Running);
        std::thread::sleep(Duration::from_millis(20));
    }
    // A new limit applies at once: 256 KB/s, where the server gives 2 MB/s over two.
    job.set_speed_limit(Some(256 * 1024));
    std::thread::sleep(Duration::from_millis(500));
    let (a, t) = (job.progress().downloaded, Instant::now());
    std::thread::sleep(Duration::from_secs(2));
    let rate = (job.progress().downloaded - a) as f64 / t.elapsed().as_secs_f64();
    assert!(rate < 512.0 * 1024.0, "the limit holds: {rate:.0} B/s");
    job.stop();
    let (p, _) = wait(&job);
    assert_eq!(p.state, State::Stopped);
    let state = mdm_engine::state::state_path(&out.path().join("Talk.f137.mp4"));
    assert!(state.exists(), "the resume state is kept");
    let job = start(tools.path(), out.path(), 2);
    job.set_speed_limit(None);
    let (p, _) = wait(&job);
    let log = log_of(&p);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert!(
        std::fs::read(out.path().join("Talk.mp4")).unwrap() == expected(),
        "resumed whole"
    );
    assert!(log.iter().any(|t| t.starts_with("Resuming: ")), "{log:?}");

    // Addresses that expire 1.5 s after their first request, on connections dropped every 512 KB
    // at 512 KB/s (3 MB over two connections, 3 s at least): yt-dlp is asked for today's, and
    // the stream goes on. Timed from the first request, not the look-up, so a busy computer's
    // slow start can't kill a link before the engine has used it.
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(
        tools.path().join("page.json"),
        page(
            "/gen/3M/v.mp4?seed=1&rate=512K&drop_after=512K&lasts_ms=1500&link={LINK}",
            "/gen/1M/a.m4a?seed=2",
            "https",
        ),
    )
    .unwrap();
    let job = start(tools.path(), out.path(), 2);
    let (p, _) = wait(&job);
    let log = log_of(&p);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert!(
        std::fs::read(out.path().join("Talk.mp4")).unwrap() == expected(),
        "whole across new addresses"
    );
    assert!(
        log.iter()
            .any(|t| t == "Asking yt-dlp for a new address for format 137"),
        "{log:?}"
    );

    // A protocol the engine doesn't speak: yt-dlp downloads the choice itself, and says why.
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(
        tools.path().join("page.json"),
        page("/gen/3M/v.mp4?seed=1", "/gen/1M/a.m4a?seed=2", "rtmp"),
    )
    .unwrap();
    let job = start(tools.path(), out.path(), 2);
    let (p, _) = wait(&job);
    let log = log_of(&p);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert_eq!(
        p.helper,
        Some(mdm_tools::streams::Helper::Protocol("rtmp".into()))
    );
    assert!(
        log.iter()
            .any(|t| t == "yt-dlp downloads this video itself: its protocol (rtmp)"),
        "{log:?}"
    );
    signed_in_checks(me, &lab, &engine);
    pieces_checks(me, &rt, &lab, &engine, &expected_audio());
    println!("fake_ytdlp: the engine's checks ok");
}

/// A live stream's recording, stopped and saved (`VideoJob::save`): yt-dlp (the fake, `live`
/// modes) records `Talk.mp4.part` as MPEG-TS through MDM's engine's look-up path (the page says
/// `is_live`), MDM's ffmpeg (the fake, as `fake-ffmpeg`) makes it an MP4. What it records shows
/// as it grows; saved, it is a video and nothing else is left; whatever fails, nothing recorded
/// is lost, and yt-dlp never starts again once saving.
fn recording_checks(me: &Path) {
    use mdm_tools::video::LiveState;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let engine = || mdm_tools::streams::Engine {
        handle: rt.handle().clone(),
        opts: mdm_engine::Options::default(),
    };
    const LIVE: &str = r#"{"id": "live", "title": "Live", "is_live": true, "ext": "mp4",
        "formats": [{"format_id": "96", "ext": "mp4", "protocol": "m3u8"}]}"#;
    // A folder for the fakes and one for the download, the fake recording as `mode`, MDM's
    // ffmpeg as `ffmpeg_mode`, its ffprobe beside it.
    let folders = |mode: &str, ffmpeg_mode: &str| {
        let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(tools.path().join("page.json"), LIVE).unwrap();
        std::fs::write(out.path().join("mode.txt"), mode).unwrap();
        std::fs::write(out.path().join("ffmpeg-mode.txt"), ffmpeg_mode).unwrap();
        let exe = fake_in(me, tools.path());
        let ffmpeg = tools
            .path()
            .join(format!("fake-ffmpeg{}", std::env::consts::EXE_SUFFIX));
        std::fs::copy(me, &ffmpeg).unwrap();
        let ffprobe = tools
            .path()
            .join(format!("fake-ffprobe{}", std::env::consts::EXE_SUFFIX));
        std::fs::copy(me, &ffprobe).unwrap();
        (tools, out, exe, ffmpeg)
    };
    let start = |exe: &Path, out: &Path, ffmpeg: Option<&Path>, by_engine: bool| {
        VideoJob::start(
            exe,
            "https://video.example/live",
            "b",
            out,
            "Talk",
            None,
            ffmpeg,
            None,
            &Default::default(),
            &Subs::default(),
            by_engine.then(engine),
        )
        .expect("started")
    };
    let until = |job: &VideoJob, what: &str, ok: &dyn Fn(&VideoJob) -> bool| {
        let end = Instant::now() + Duration::from_secs(60);
        while !ok(job) {
            let p = job.progress();
            assert!(
                Instant::now() < end && p.state == State::Running,
                "{what}: {:?} {:?}",
                p.state,
                log_of(&p)
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    // Recording with something in: what the tests that save and expect an MP4 wait for (a
    // recording is shown from the moment the recorder opens its file, its first block later).
    let recording = |job: &VideoJob| {
        job.live_state() == Some(LiveState::Recording) && job.progress().recorded > 0
    };
    // Recording, and enough of it that what a check tells apart doesn't hang on how much was in
    // when it saved (the watcher reads the size once a second while the fake writes a block of
    // 65,424 bytes every tenth: anything from one block to ten). The fake ffprobe's `short` modes
    // say the MP4 (or its sound) plays half as long as the recording, and MDM takes an MP4 up to
    // a second shorter as whole (`SLACK` in video.rs): up to 2 s recorded (three blocks), the
    // missing half is within that second and the MP4 counts as whole. 3 s of the fake's playback
    // (five blocks) is past it, as past the 64 KiB `tiny` checks (two blocks) and the two rounds
    // `live-split` checks. What is recorded only grows until yt-dlp is stopped.
    let enough =
        |job: &VideoJob| recording(job) && job.progress().recorded as f64 >= 3.0 * BYTES_PER_SECOND;
    // Recording, whatever has been written.
    let opened = |job: &VideoJob| job.live_state() == Some(LiveState::Recording);
    // For a while (twice the watcher's second) nothing says it records: it isn't, while the
    // fake holds its file back. Not a wait for anything, the fake's files are what synchronize.
    let not_recording = |job: &VideoJob, what: &str| {
        let end = Instant::now() + Duration::from_millis(2500);
        while Instant::now() < end {
            let p = job.progress();
            assert_eq!(p.state, State::Running, "{what}: {:?}", log_of(&p));
            assert!(!p.recording, "{what}");
            assert_eq!(job.live_state(), None, "{what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let ended = |job: &VideoJob| {
        let end = Instant::now() + Duration::from_secs(120);
        loop {
            let p = job.progress();
            if p.state != State::Running {
                return p;
            }
            assert!(Instant::now() < end, "still running: {:?}", log_of(&p));
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let count = |file: PathBuf| {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|t| t.trim().parse::<u32>().ok())
            .unwrap_or(0)
    };
    let names = |out: &Path| {
        let mut n: Vec<String> = std::fs::read_dir(out)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.ends_with(".txt") && !n.ends_with(".log"))
            .collect();
        n.sort();
        n
    };
    let ts = |len: usize| {
        let mut packet = [0u8; 188];
        packet[0] = 0x47;
        packet.repeat(len.div_ceil(188))
    };

    // Saved: an MP4, nothing else left. What it records shows, growing, and is gone once over;
    // saved twice and stopped while saving, it is saved once, and yt-dlp ran once.
    let (_tools, out, exe, ffmpeg) = folders("live", "");
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "recording", &recording);
    let first = job.progress().recorded;
    until(&job, "grows", &|j: &VideoJob| j.progress().recorded > first);
    assert_eq!(job.helper(), Some(mdm_tools::streams::Helper::Live));
    job.save();
    assert_eq!(job.live_state(), Some(LiveState::Saving));
    assert_eq!(job.progress().state, State::Running, "saving: not over yet");
    job.save();
    job.stop();
    assert_eq!(
        job.live_state(),
        Some(LiveState::Saving),
        "a Stop changes nothing"
    );
    let p = ended(&job);
    let log = log_of(&p);
    assert_eq!(p.state, State::Done(out.path().join("Talk.mp4")), "{log:?}");
    assert_eq!(names(out.path()), ["Talk.mp4"], "no .part, no temp");
    let mp4 = std::fs::read(out.path().join("Talk.mp4")).unwrap();
    assert!(
        mp4.starts_with(b"FAKEMP4 ") && mp4[8] == 0x47,
        "made by ffmpeg"
    );
    assert!(mp4.len() as u64 > first, "all of it");
    assert_eq!(p.downloaded, mp4.len() as u64);
    assert_eq!(p.recorded, 0, "no longer recording");
    assert_eq!(job.live_state(), None);
    assert_eq!(count(out.path().join("tries.txt")), 1, "yt-dlp ran once");
    assert_eq!(count(out.path().join("ffmpeg-runs.txt")), 1, "saved once");
    job.stop();
    assert_eq!(job.progress().state, p.state, "done stays done");
    println!(
        "recording_checks: saved {} bytes as Talk.mp4 ({first} recorded at first)",
        mp4.len()
    );

    // `Talk.mp4` there already: the recording beside it, out of MDM's numbering.
    let (_tools, out, exe, ffmpeg) = folders("live", "");
    std::fs::write(out.path().join("Talk.mp4"), b"another").unwrap();
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "recording", &recording);
    job.save();
    let p = ended(&job);
    assert_eq!(
        p.state,
        State::Done(out.path().join("Talk (recording 1).mp4")),
        "{:?}",
        log_of(&p)
    );
    assert_eq!(
        std::fs::read(out.path().join("Talk.mp4")).unwrap(),
        b"another"
    );
    assert_eq!(names(out.path()), ["Talk (recording 1).mp4", "Talk.mp4"]);

    // ffmpeg fails: kept as the MPEG-TS it is, its half-made MP4 gone.
    let (_tools, out, exe, ffmpeg) = folders("live", "fail");
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "recording", &recording);
    job.save();
    let p = ended(&job);
    let log = log_of(&p);
    assert_eq!(p.state, State::Done(out.path().join("Talk.ts")), "{log:?}");
    assert_eq!(names(out.path()), ["Talk.ts"]);
    assert!(
        log.iter()
            .any(|t| t.starts_with("The recording couldn't be made an MP4 (")),
        "{log:?}"
    );

    // ffmpeg's file far smaller than the recording: not taken, the recording kept whole.
    let (_tools, out, exe, ffmpeg) = folders("live", "tiny");
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "enough recorded", &enough);
    job.save();
    let p = ended(&job);
    assert_eq!(p.state, State::Done(out.path().join("Talk.ts")));
    assert_eq!(names(out.path()), ["Talk.ts"]);
    let kept = std::fs::read(out.path().join("Talk.ts")).unwrap();
    assert!(kept.len() >= 64 << 10 && kept[0] == 0x47, "{}", kept.len());

    // Not MPEG-TS, and ffmpeg failed: never a file that isn't one; the recording stays as it
    // is, and the error says where.
    let (_tools, out, exe, ffmpeg) = folders("live-raw", "fail");
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "recording", &recording);
    job.save();
    let p = ended(&job);
    let State::Failed(e) = &p.state else {
        panic!("{:?}", p.state)
    };
    assert!(
        e.contains("Talk.mp4.part") && e.contains("isn't MPEG-TS"),
        "{e}"
    );
    assert_eq!(names(out.path()), ["Talk.mp4.part"]);
    assert!(
        std::fs::metadata(out.path().join("Talk.mp4.part"))
            .unwrap()
            .len()
            > 0
    );

    // Saved before anything is recorded: a Stop.
    let (_tools, out, exe, ffmpeg) = folders("live", "");
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    job.save();
    assert_eq!(ended(&job).state, State::Stopped);
    assert_eq!(count(out.path().join("ffmpeg-runs.txt")), 0);

    // The recorder connects first and opens its file later (`live-late`, the test says when with
    // `open.txt`, then `go.txt` for its first block): nothing is recording while there is no
    // file. A 0-byte `.part` an earlier run left (a stop in its first seconds) is removed before
    // yt-dlp starts, so the new recording shows once its file is made, though empty (and not
    // later, once it has grown).
    let (_tools, out, exe, ffmpeg) = folders("live-late", "");
    let part = out.path().join("Talk.mp4.part");
    std::fs::write(&part, b"").unwrap();
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "started", &|_: &VideoJob| {
        count(out.path().join("tries.txt")) > 0
    });
    not_recording(&job, "an earlier empty .part, before open.txt");
    assert!(!part.exists(), "the earlier empty file is removed");
    assert!(
        log_of(&job.progress()).contains(
            &"Removed the empty unfinished file Talk.mp4.part left by an earlier run".to_owned()
        ),
        "{:?}",
        log_of(&job.progress())
    );
    std::fs::write(out.path().join("open.txt"), "").unwrap();
    until(&job, "opened", &opened);
    let p = job.progress();
    assert_eq!((p.recorded, p.recording), (0, true));
    assert_eq!(std::fs::metadata(&part).unwrap().len(), 0);
    std::fs::write(out.path().join("go.txt"), "").unwrap();
    until(&job, "recording", &recording);
    job.stop();
    assert_eq!(ended(&job).state, State::Stopped);

    // No file yet, then an empty one: recording at 0 bytes. Stop and save keeps nothing, leaves
    // the download stopped (to resume), the empty file as it is, no MP4, and ffmpeg unused.
    let (_tools, out, exe, ffmpeg) = folders("live-late", "");
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "started", &|_: &VideoJob| {
        count(out.path().join("tries.txt")) > 0
    });
    not_recording(&job, "no file yet");
    assert!(!out.path().join("Talk.mp4.part").exists());
    std::fs::write(out.path().join("open.txt"), "").unwrap();
    until(&job, "opened", &opened);
    let p = job.progress();
    assert_eq!((p.recorded, p.recording), (0, true));
    assert_eq!(
        std::fs::metadata(out.path().join("Talk.mp4.part"))
            .unwrap()
            .len(),
        0
    );
    job.save();
    assert_eq!(
        job.live_state(),
        Some(LiveState::Saving),
        "saved, not just stopped"
    );
    let p = ended(&job);
    assert_eq!(p.state, State::Stopped, "{:?}", log_of(&p));
    assert_eq!(
        names(out.path()),
        ["Talk.mp4.part"],
        "no MP4, nothing deleted"
    );
    assert_eq!(
        std::fs::metadata(out.path().join("Talk.mp4.part"))
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        count(out.path().join("ffmpeg-runs.txt")),
        0,
        "ffmpeg not run"
    );
    assert_eq!(count(out.path().join("tries.txt")), 1, "yt-dlp ran once");
    assert_eq!((p.recorded, p.recording), (0, false));
    assert_eq!(job.live_state(), None);

    // Another one: the file opened, then its first block: what is recorded shows, and Stop and
    // save makes the MP4 as for any recording.
    let (_tools, out, exe, ffmpeg) = folders("live-late", "");
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    std::fs::write(out.path().join("open.txt"), "").unwrap();
    until(&job, "opened", &opened);
    std::fs::write(out.path().join("go.txt"), "").unwrap();
    until(&job, "recorded", &recording);
    assert!(job.progress().recorded > 0);
    job.save();
    let p = ended(&job);
    assert_eq!(
        p.state,
        State::Done(out.path().join("Talk.mp4")),
        "{:?}",
        log_of(&p)
    );
    assert_eq!(names(out.path()), ["Talk.mp4"]);
    assert_eq!(count(out.path().join("ffmpeg-runs.txt")), 1);
    assert_eq!((p.recorded, p.recording), (0, false));

    // Without MDM's engine nothing says it's live: Save is a Stop, the recording stays.
    let (_tools, out, exe, ffmpeg) = folders("live", "");
    let job = start(&exe, out.path(), Some(&ffmpeg), false);
    until(&job, "recorded", &|_: &VideoJob| {
        std::fs::metadata(out.path().join("Talk.mp4.part")).is_ok_and(|m| m.len() > 0)
    });
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(job.helper(), None);
    assert_eq!(job.live_state(), None);
    assert_eq!(job.progress().recorded, 0);
    job.save();
    assert_eq!(ended(&job).state, State::Stopped);
    assert_eq!(names(out.path()), ["Talk.mp4.part"]);
    assert_eq!(count(out.path().join("ffmpeg-runs.txt")), 0);

    // Saved while the page is looked up again between two runs (the first failed): a Stop,
    // and yt-dlp doesn't start again.
    let (tools, out, exe, ffmpeg) = folders("live-dns-once", "");
    std::fs::write(tools.path().join("slow-second-look-up"), "").unwrap();
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "looking up again", &|j: &VideoJob| {
        log_of(&j.progress())
            .iter()
            .any(|t| t.ends_with("couldn't be reached; looking the page up again without it"))
    });
    assert_eq!(job.live_state(), None, "between runs: not recording");
    job.save();
    let p = ended(&job);
    assert_eq!(p.state, State::Stopped, "{:?}", log_of(&p));
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(
        count(out.path().join("tries.txt")),
        1,
        "yt-dlp didn't start again"
    );
    assert_eq!(names(out.path()), ["Talk.mp4.part"]);

    // An earlier recording left there (Cancel, then Resume): kept aside before yt-dlp records
    // over it, each time under a name of its own, never written over.
    let (_tools, out, exe, ffmpeg) = folders("live", "");
    let earlier = ts(100_000);
    std::fs::write(out.path().join("Talk.mp4.part"), &earlier).unwrap();
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "recording", &recording);
    let log = log_of(&job.progress());
    assert!(
        log.iter()
            .any(|t| t == "The earlier recording was kept as Talk (recording 1).ts"),
        "{log:?}"
    );
    job.stop();
    assert_eq!(ended(&job).state, State::Stopped);
    let second = std::fs::read(out.path().join("Talk.mp4.part")).unwrap();
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "recording", &recording);
    job.stop();
    assert_eq!(ended(&job).state, State::Stopped);
    let read = |n: &str| std::fs::read(out.path().join(n)).unwrap();
    assert_eq!(read("Talk (recording 1).ts"), earlier);
    assert_eq!(read("Talk (recording 2).ts"), second);
    assert_eq!(count(out.path().join("tries.txt")), 2);
    // Another download of the same name, numbered by MDM (`Talk (2)`), in the same folder:
    // none of these are its files, nor this download's own.
    std::fs::write(out.path().join("Talk (2).mp4"), b"another").unwrap();
    let theirs = mdm_tools::video::own_files(out.path(), "Talk (2)", true);
    assert_eq!(theirs, [out.path().join("Talk (2).mp4")]);
    let ours = mdm_tools::video::own_files(out.path(), "Talk", true);
    assert!(
        ours.iter()
            .all(|f| !f.to_string_lossy().contains("(recording")),
        "{ours:?}"
    );

    // The MP4 plays shorter than the recording (ffprobe says so): saved all the same, and the
    // recording kept beside it as MPEG-TS, byte for byte, the log saying why.
    let (_tools, out, exe, ffmpeg) = folders("live", "");
    std::fs::write(out.path().join("ffprobe-mode.txt"), "short").unwrap();
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "enough recorded", &enough);
    job.save();
    let p = ended(&job);
    let log = log_of(&p);
    assert_eq!(p.state, State::Done(out.path().join("Talk.mp4")), "{log:?}");
    // Out of the download's own names: Replace would take `Talk.ts` with it.
    let kept = out.path().join("Talk (recording 1).ts");
    assert_eq!(names(out.path()), ["Talk (recording 1).ts", "Talk.mp4"]);
    let mp4 = std::fs::read(out.path().join("Talk.mp4")).unwrap();
    assert_eq!(std::fs::read(&kept).unwrap(), mp4[8..]);
    assert!(
        log.iter().any(|t| t
            == "The MP4 doesn't hold all of the recording (it plays shorter, or one of its streams starts later or ends sooner), so the recording is kept beside it too: Talk (recording 1).ts"),
        "{log:?}"
    );
    assert!(!mdm_tools::video::own_files(out.path(), "Talk", true).contains(&kept));

    // The MP4 plays to the end but its sound doesn't: not all of it.
    // No ffprobe (or one that can't tell, or only estimated): whole when the MP4 is nine tenths
    // of the recording's size at least; four fifths isn't, and the recording stays beside it.
    for (ffprobe, ffmpeg_mode, whole) in [
        ("short-sound", "", false),
        ("none", "", true),
        ("none", "short", false),
        ("fail", "", true),
        ("fail", "short", false),
        ("estimate", "", true),
        ("estimate", "short", false),
    ] {
        let (tools, out, exe, ffmpeg) = folders("live", ffmpeg_mode);
        match ffprobe {
            "none" => std::fs::remove_file(
                tools
                    .path()
                    .join(format!("fake-ffprobe{}", std::env::consts::EXE_SUFFIX)),
            )
            .unwrap(),
            mode => std::fs::write(out.path().join("ffprobe-mode.txt"), mode).unwrap(),
        }
        let job = start(&exe, out.path(), Some(&ffmpeg), true);
        until(&job, "enough recorded", &enough);
        job.save();
        let p = ended(&job);
        let log = log_of(&p);
        assert_eq!(p.state, State::Done(out.path().join("Talk.mp4")), "{log:?}");
        let mp4 = std::fs::read(out.path().join("Talk.mp4")).unwrap();
        if whole {
            assert_eq!(names(out.path()), ["Talk.mp4"], "{ffprobe} {ffmpeg_mode}");
        } else {
            assert_eq!(
                names(out.path()),
                ["Talk (recording 1).ts", "Talk.mp4"],
                "{ffprobe}"
            );
            let kept = std::fs::read(out.path().join("Talk (recording 1).ts")).unwrap();
            let shown = if ffmpeg_mode == "short" {
                kept.len() * 4 / 5
            } else {
                kept.len()
            };
            assert_eq!(mp4[8..], kept[..shown], "{ffprobe}");
            assert!(
                log.iter()
                    .any(|t| t.starts_with("The MP4 doesn't hold all of the recording (it plays shorter, or one of its streams starts later or ends sooner)")),
                "{log:?}"
            );
        }
    }

    // The picture and the sound recorded apart (each a stream's own part): one MP4 of both,
    // the parts gone, the log naming them.
    let (_tools, out, exe, ffmpeg) = folders("live-split", "");
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "enough recorded", &enough);
    let (video, audio) = (
        out.path().join("Talk.f1.mp4.part"),
        out.path().join("Talk.f2.m4a.part"),
    );
    let mut own = mdm_tools::video::own_files(out.path(), "Talk", false);
    own.sort();
    assert_eq!(own, [video.clone(), audio.clone()], "yt-dlp's own files");
    job.save();
    let p = ended(&job);
    let log = log_of(&p);
    assert_eq!(p.state, State::Done(out.path().join("Talk.mp4")), "{log:?}");
    assert_eq!(names(out.path()), ["Talk.mp4"]);
    let mp4 = std::fs::read(out.path().join("Talk.mp4")).unwrap();
    // The picture's then the sound's (a third of it), each in whole writes of the fake's.
    let (v, a) = (348 * 188, 116 * 188);
    assert!(
        [0, v].contains(&((mp4.len() - 8) % (v + a))) && mp4.len() > 8 + v + a,
        "both in: {}",
        mp4.len()
    );
    assert!(
        log.iter()
            .any(|t| t == "The recording came in 2 parts: Talk.f1.mp4.part, Talk.f2.m4a.part"),
        "{log:?}"
    );

    // Recorded apart, and ffmpeg fails: nothing renamed, both kept as they are, named.
    let (_tools, out, exe, ffmpeg) = folders("live-split", "fail");
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "recording", &recording);
    job.save();
    let p = ended(&job);
    let State::Failed(e) = &p.state else {
        panic!("{:?}", p.state)
    };
    assert!(
        e.starts_with("The recording's 2 parts couldn't be joined into an MP4 (")
            && e.contains("Talk.f1.mp4.part")
            && e.contains("Talk.f2.m4a.part"),
        "{e}"
    );
    assert_eq!(names(out.path()), ["Talk.f1.mp4.part", "Talk.f2.m4a.part"]);
    let kept: Vec<Vec<u8>> = ["Talk.f1.mp4.part", "Talk.f2.m4a.part"]
        .iter()
        .map(|part| std::fs::read(out.path().join(part)).unwrap())
        .collect();
    assert!(kept.iter().all(|b| !b.is_empty() && b[0] == 0x47));
    // Resumed once the stream had ended (a video now, not live, yt-dlp downloading it as any
    // other): both parts kept aside before anything runs, byte for byte.
    std::fs::write(out.path().join("mode.txt"), "").unwrap();
    let tools = tempfile::tempdir().unwrap();
    let job = VideoJob::start(
        &fake_in(me, tools.path()),
        "https://video.example/talk",
        "b",
        out.path(),
        "Talk",
        None,
        Some(&ffmpeg),
        None,
        &Default::default(),
        &Subs::default(),
        None,
    )
    .expect("started");
    let p = ended(&job);
    assert_eq!(
        p.state,
        State::Done(out.path().join("Talk.mp4")),
        "{:?}",
        log_of(&p)
    );
    assert_eq!(
        names(out.path()),
        ["Talk (recording 1).ts", "Talk (recording 2).ts", "Talk.mp4"]
    );
    assert_eq!(
        std::fs::read(out.path().join("Talk (recording 1).ts")).unwrap(),
        kept[0]
    );
    assert_eq!(
        std::fs::read(out.path().join("Talk (recording 2).ts")).unwrap(),
        kept[1]
    );

    // Recorded apart before (Cancel, then Resume): each part kept aside on its own.
    let (_tools, out, exe, ffmpeg) = folders("live-split", "");
    let (earlier_video, earlier_audio) = (ts(90_000), ts(30_000));
    std::fs::write(out.path().join("Talk.f1.mp4.part"), &earlier_video).unwrap();
    std::fs::write(out.path().join("Talk.f2.m4a.part"), &earlier_audio).unwrap();
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    until(&job, "recording", &recording);
    job.stop();
    assert_eq!(ended(&job).state, State::Stopped);
    let read = |n: &str| std::fs::read(out.path().join(n)).unwrap();
    assert_eq!(read("Talk (recording 1).ts"), earlier_video);
    assert_eq!(read("Talk (recording 2).ts"), earlier_audio);

    // Resumed once the stream had ended (a video now, downloaded as any other): its earlier
    // recording, a `.part` with no resume state of yt-dlp's, is kept aside, never downloaded on
    // to; with that state beside it, it is a download to go on with, left alone.
    for resumable in [false, true] {
        let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(out.path().join("mode.txt"), "").unwrap();
        let exe = fake_in(me, tools.path());
        let earlier = ts(50_000);
        let part = out.path().join("Talk.mp4.part");
        std::fs::write(&part, &earlier).unwrap();
        if resumable {
            std::fs::write(out.path().join("Talk.mp4.ytdl"), b"{}").unwrap();
        }
        let job = VideoJob::start(
            &exe,
            "https://video.example/talk",
            "b",
            out.path(),
            "Talk",
            None,
            None,
            None,
            &Default::default(),
            &Subs::default(),
            None,
        )
        .expect("started");
        let p = ended(&job);
        let log = log_of(&p);
        assert_eq!(p.state, State::Done(out.path().join("Talk.mp4")), "{log:?}");
        if resumable {
            assert_eq!(std::fs::read(&part).unwrap(), earlier, "left alone");
            assert_eq!(
                names(out.path()),
                ["Talk.mp4", "Talk.mp4.part", "Talk.mp4.ytdl"]
            );
        } else {
            assert_eq!(
                std::fs::read(out.path().join("Talk (recording 1).ts")).unwrap(),
                earlier
            );
            assert_eq!(names(out.path()), ["Talk (recording 1).ts", "Talk.mp4"]);
            assert!(
                log.iter()
                    .any(|t| t == "The earlier recording was kept as Talk (recording 1).ts"),
                "{log:?}"
            );
        }
    }

    // The live stream ends by itself while recorded: yt-dlp's own file, no ffmpeg of MDM's.
    // The size shown (the larger of what came and what is recorded) never falls back to
    // nothing on the way from the recording to the end.
    let (_tools, out, exe, ffmpeg) = folders("live-ends", "");
    let job = start(&exe, out.path(), Some(&ffmpeg), true);
    let end = Instant::now() + Duration::from_secs(60);
    let (mut seen, mut dips, mut polls) = (false, 0, 0);
    let p = loop {
        let p = job.progress();
        if p.state != State::Running {
            break p;
        }
        if p.downloaded.max(p.recorded) > 0 {
            seen = true;
        } else if seen {
            dips += 1;
        }
        polls += 1;
        assert!(Instant::now() < end, "still running: {:?}", log_of(&p));
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(seen, "the recording was shown");
    assert_eq!(
        dips, 0,
        "the size shown fell to 0 in {dips} of {polls} looks"
    );
    assert_eq!(
        p.state,
        State::Done(out.path().join("Talk.mp4")),
        "{:?}",
        log_of(&p)
    );
    let mp4 = std::fs::read(out.path().join("Talk.mp4")).unwrap();
    assert!(mp4.starts_with(b"FAKEMP4 "), "yt-dlp's own");
    assert_eq!((p.downloaded, p.recorded), (mp4.len() as u64, 0));
    assert_eq!(names(out.path()), ["Talk.mp4"]);
    assert_eq!(
        count(out.path().join("ffmpeg-runs.txt")),
        0,
        "MDM's ffmpeg not run"
    );
    assert_eq!(count(out.path().join("tries.txt")), 1);
    println!("recording_checks: a live stream that ended by itself, {polls} looks, no dip");

    // Windows: an earlier recording another program holds (it can't be renamed): yt-dlp isn't
    // started, the error names it, and it stays as it is.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let (_tools, out, exe, ffmpeg) = folders("live", "");
        let part = out.path().join("Talk.mp4.part");
        std::fs::write(&part, &earlier).unwrap();
        // Shared for reading and writing, not for deleting (or renaming).
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2)
            .open(&part)
            .unwrap();
        let job = start(&exe, out.path(), Some(&ffmpeg), true);
        let p = ended(&job);
        drop(held);
        let State::Failed(e) = &p.state else {
            panic!("{:?}", p.state)
        };
        assert!(e.contains("Talk.mp4.part"), "{e}");
        assert_eq!(count(out.path().join("tries.txt")), 0, "yt-dlp not started");
        assert_eq!(std::fs::read(&part).unwrap(), earlier);
        assert_eq!(names(out.path()), ["Talk.mp4.part"]);
    }

    // Windows: another program holds the MP4 ffmpeg made (it can't be renamed, nor removed):
    // the recording is kept as the MPEG-TS it is, byte for byte, and the leftover stays. Held
    // by this program, not a child of ffmpeg's: ffmpeg runs in a job that ends all it started.
    #[cfg(windows)]
    {
        use std::io::Read;
        use std::os::windows::fs::OpenOptionsExt;
        let (_tools, out, exe, ffmpeg) = folders("live", "linger");
        let job = start(&exe, out.path(), Some(&ffmpeg), true);
        until(&job, "recording", &recording);
        job.save();
        let temp = out.path().join("Talk.temp.mp4");
        let end = Instant::now() + Duration::from_secs(60);
        let mut held = loop {
            if std::fs::metadata(&temp).is_ok_and(|m| m.len() > 8) {
                break std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0x1 | 0x2)
                    .open(&temp)
                    .unwrap();
            }
            assert!(Instant::now() < end, "no MP4 made");
            assert_eq!(
                job.progress().state,
                State::Running,
                "ended before it was held"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        let p = ended(&job);
        let log = log_of(&p);
        let mut made = Vec::new();
        held.read_to_end(&mut made).unwrap();
        drop(held);
        assert_eq!(p.state, State::Done(out.path().join("Talk.ts")), "{log:?}");
        assert_eq!(
            std::fs::read(out.path().join("Talk.ts")).unwrap(),
            made[8..],
            "the recording as ffmpeg read it"
        );
        assert_eq!(names(out.path()), ["Talk.temp.mp4", "Talk.ts"]);
        assert!(
            log.iter()
                .any(|t| t.starts_with("The recording couldn't be made an MP4 (")
                    && t.contains("Talk.temp.mp4 couldn't be renamed")),
            "{log:?}"
        );
    }

    // Windows: another program holds the recording once its MP4 is made (it can't be
    // removed): saved all the same, the log says what is left.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let (_tools, out, exe, ffmpeg) = folders("live", "");
        let job = start(&exe, out.path(), Some(&ffmpeg), true);
        until(&job, "recording", &recording);
        let part = out.path().join("Talk.mp4.part");
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2)
            .open(&part)
            .unwrap();
        job.save();
        let p = ended(&job);
        drop(held);
        let log = log_of(&p);
        assert_eq!(p.state, State::Done(out.path().join("Talk.mp4")), "{log:?}");
        assert!(
            log.iter().any(
                |t| t.starts_with("The recording is saved, but its unfinished file")
                    && t.contains("Talk.mp4.part couldn't be removed")
            ),
            "{log:?}"
        );
        let mp4 = std::fs::read(out.path().join("Talk.mp4")).unwrap();
        assert_eq!(
            mp4[8..],
            std::fs::read(&part).unwrap(),
            "all of it in the MP4"
        );
        assert_eq!(names(out.path()), ["Talk.mp4", "Talk.mp4.part"]);
    }
    // Windows: the MP4 is shorter than the recording, and another program holds the recording
    // (it can't be renamed aside): saved all the same, the recording left as it is, never
    // removed, and the log says it couldn't be renamed.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let (_tools, out, exe, ffmpeg) = folders("live", "");
        std::fs::write(out.path().join("ffprobe-mode.txt"), "short").unwrap();
        let job = start(&exe, out.path(), Some(&ffmpeg), true);
        until(&job, "enough recorded", &enough);
        let part = out.path().join("Talk.mp4.part");
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2)
            .open(&part)
            .unwrap();
        job.save();
        let p = ended(&job);
        drop(held);
        let log = log_of(&p);
        assert_eq!(p.state, State::Done(out.path().join("Talk.mp4")), "{log:?}");
        assert!(
            log.iter().any(|t| t
                .starts_with("The MP4 doesn't hold all of the recording (it plays shorter, or one of its streams starts later or ends sooner), and the recording ")
                && t.contains("Talk.mp4.part couldn't be renamed (")
                && t.ends_with("it stays as Talk.mp4.part")),
            "{log:?}"
        );
        assert!(
            !log.iter().any(|t| t.contains("kept beside it too")),
            "{log:?}"
        );
        let mp4 = std::fs::read(out.path().join("Talk.mp4")).unwrap();
        assert_eq!(
            std::fs::read(&part).unwrap(),
            mp4[8..],
            "the recording as it was"
        );
        assert_eq!(names(out.path()), ["Talk.mp4", "Talk.mp4.part"]);
    }
    println!("fake_ytdlp: the recordings' checks ok");
}

/// #1102: an X post with a long text, as the report's: its video saved in a deep folder (here,
/// under the build's own folder) under a name shortened so that every file MDM's engine and
/// yt-dlp make for it stays within Windows' 259 (and names within 255 everywhere), both when the
/// engine downloads its streams (an HLS sound in pieces) and when yt-dlp does (its `.part` and
/// `.ytdl`). A name not shortened is refused at once, and a path yt-dlp couldn't make is named
/// as too long, not as a missing folder.
fn long_name_checks(me: &Path) {
    use mdm_labserver::{Config, LabServer};
    const TITLE: &str = "Kira on X: \"This is f*cking insane. Nvidia CEO Jensen Huang on what he expects from an engineer he pays $500,000 a year: \"If that $500,000 engineer did not consume at least $250,000 worth of tokens\" He'd be deeply alarmed";
    let rt = tokio::runtime::Runtime::new().unwrap();
    let lab = rt.block_on(LabServer::start(Config::default())).unwrap();
    let engine = mdm_tools::streams::Engine {
        handle: rt.handle().clone(),
        opts: mdm_engine::Options {
            max_connections: 2,
            retry_delay: Duration::from_millis(50),
            ..Default::default()
        },
    };
    let page = |protocol: &str, audio_id: &str| {
        format!(
            r#"{{"id": "1", "title": "{}", "ext": "mp4", "formats": [
            {{"format_id": "hls-2176", "ext": "mp4", "protocol": "{protocol}", "filesize": 1048576,
              "vcodec": "avc1", "acodec": "none", "url": "{}"}},
            {{"format_id": "{audio_id}", "ext": "mp4", "protocol": "{}",
              "vcodec": "none", "acodec": "mp4a", "url": "{}"}}]}}"#,
            TITLE.replace('"', "\\\""),
            lab.url("/gen/1M/v.mp4?seed=1"),
            if protocol == "https" {
                "m3u8_native"
            } else {
                protocol
            },
            lab.url("/hls/720p/index.m3u8")
        )
    };
    // Folders 100 characters long (89, and a temporary folder's `\.tmpXXXXXX` in it), on this
    // computer's disk under the build's folder.
    let base = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let pad = 89usize
        .saturating_sub(base.path().as_os_str().len() + 1)
        .max(1);
    let folder = base.path().join("d".repeat(pad));
    std::fs::create_dir_all(&folder).unwrap();
    let name = format!("{}.mp4", mdm_engine::naming::sanitize(TITLE));
    let units = |p: &Path| {
        let s = p.to_string_lossy();
        if cfg!(windows) {
            s.encode_utf16().count()
        } else {
            s.len()
        }
    };
    let longest = |dir: &Path| -> usize {
        fn walk(d: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                out.push(e.path());
                if e.path().is_dir() {
                    walk(&e.path(), out);
                }
            }
        }
        let mut all = Vec::new();
        walk(dir, &mut all);
        all.iter().map(|p| units(p)).max().unwrap_or(0)
    };
    for protocol in ["https", "rtmp"] {
        let (tools, out) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir_in(&folder).unwrap(),
        );
        let dir = out.path().to_owned();
        std::fs::write(
            tools.path().join("page.json"),
            page(protocol, "audio-128000-Audio"),
        )
        .unwrap();
        let dest = mdm_engine::naming::stem_destination(&dir, &name, |_| false);
        let stem = dest.file_stem().unwrap().to_string_lossy().into_owned();
        assert!(stem.len() < 221, "shortened: {stem}");
        assert!(
            mdm_engine::naming::sanitize(TITLE).starts_with(&stem),
            "{stem}"
        );
        let exe = fake_in(me, tools.path());
        let job = VideoJob::start(
            &exe,
            "https://x.example/kira/status/1",
            "bv*+ba/b",
            &dir,
            &stem,
            None,
            None,
            None,
            &Default::default(),
            &Subs::default(),
            Some(engine.clone()),
        )
        .expect("started");
        let end = Instant::now() + Duration::from_secs(120);
        let mut most = 0;
        let p = loop {
            most = most.max(longest(&dir));
            let p = job.progress();
            if p.state != State::Running {
                break p;
            }
            assert!(Instant::now() < end, "still running: {:?}", log_of(&p));
            std::thread::sleep(Duration::from_millis(20));
        };
        let log = log_of(&p);
        let State::Done(saved) = &p.state else {
            panic!("{protocol}: {:?}: {log:?}", p.state);
        };
        assert_eq!(saved, &dest, "{protocol}");
        if protocol == "https" {
            assert!(p.helper.is_none(), "the engine's: {:?}", p.helper);
        }
        if cfg!(windows) {
            assert!(units(saved) <= 259 && most <= 259, "{protocol}: {most}");
        }
        println!(
            "fake_ytdlp: #1102 ({protocol}): saved as {} ({} long, the longest file on the way {most})",
            saved.display(),
            units(saved)
        );
    }

    // The title's name, not shortened: refused before yt-dlp is asked anything.
    let dir = tempfile::tempdir_in(&folder).unwrap();
    let whole = mdm_engine::naming::sanitize(TITLE);
    let refused = VideoJob::start(
        Path::new("not-run"),
        "https://x.example/kira/status/1",
        "bv*+ba/b",
        dir.path(),
        &whole,
        None,
        None,
        None,
        &Default::default(),
        &Subs::default(),
        None,
    );
    assert!(
        matches!(refused, Err(mdm_tools::proc::ProcError::PathTooLong(_))),
        "refused at once"
    );

    // A format id longer than the room kept for one (rare): yt-dlp can't make its file, and
    // the path is named as too long (Windows, where the path limit is).
    if cfg!(windows) {
        let (tools, out) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir_in(&folder).unwrap(),
        );
        std::fs::write(
            tools.path().join("page.json"),
            page("rtmp", &"long-id-".repeat(10)),
        )
        .unwrap();
        let dest = mdm_engine::naming::stem_destination(out.path(), &name, |_| false);
        let stem = dest.file_stem().unwrap().to_string_lossy().into_owned();
        let exe = fake_in(me, tools.path());
        let job = VideoJob::start(
            &exe,
            "https://x.example/kira/status/1",
            "bv*+ba/b",
            out.path(),
            &stem,
            None,
            None,
            None,
            &Default::default(),
            &Subs::default(),
            Some(engine.clone()),
        )
        .expect("started");
        let end = Instant::now() + Duration::from_secs(120);
        let p = loop {
            let p = job.progress();
            if p.state != State::Running {
                break p;
            }
            assert!(Instant::now() < end, "still running");
            std::thread::sleep(Duration::from_millis(50));
        };
        let State::Failed(error) = &p.state else {
            panic!("{:?}", p.state);
        };
        assert!(error.contains("[Errno 2]"), "{error}");
        let path = mdm_tools::video::too_long(error).expect("named as too long");
        assert!(units(&path) > 259, "{}", path.display());
        assert!(path.starts_with(out.path()), "{}", path.display());
    }
    println!("fake_ytdlp: #1102's long names ok");
}

/// The sound every page of the engine's checks has (format 140, the lab's `/gen/1M/a.m4a`).
fn expected_audio() -> Vec<u8> {
    let mut a = vec![0; 1 << 20];
    mdm_labserver::content::fill(2, 0, &mut a);
    a
}

/// #769: streams in pieces through the engine: an HLS playlist (plain, and AES-128 with the
/// playlist's key), DASH's pieces as yt-dlp lists them, and pieces whose addresses expire,
/// asked of yt-dlp again. Each comes whole, in order, joined as yt-dlp's own downloaders join
/// them, then merged with the sound.
fn pieces_checks(
    me: &Path,
    rt: &tokio::runtime::Runtime,
    lab: &mdm_labserver::LabServer,
    engine: &dyn Fn(u32) -> mdm_tools::streams::Engine,
    audio: &[u8],
) {
    let get = |path: &str| -> Vec<u8> {
        let url = lab.url(path);
        rt.block_on(async {
            reqwest::get(url)
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
                .to_vec()
        })
    };
    let audio_format = format!(
        r#"{{"format_id": "140", "ext": "m4a", "protocol": "https", "filesize": {},
          "vcodec": "none", "acodec": "mp4a", "url": "{}"}}"#,
        1 << 20,
        lab.url("/gen/1M/a.m4a?seed=2")
    );
    let run = |video: String, connections: u32| {
        let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(
            tools.path().join("page.json"),
            format!(
                r#"{{"id": "talk", "title": "Talk", "ext": "mp4", "formats": [{video}, {audio_format}]}}"#
            ),
        )
        .unwrap();
        let exe = fake_in(me, tools.path());
        let job = VideoJob::start(
            &exe,
            "https://video.example/talk",
            "bv*+ba/b",
            out.path(),
            "Talk",
            None,
            None,
            None,
            &Default::default(),
            &Subs::default(),
            Some(engine(connections)),
        )
        .expect("started");
        let end = Instant::now() + Duration::from_secs(120);
        let mut most = 0;
        let p = loop {
            let p = job.progress();
            if let Some(e) = &p.engine {
                most = most.max(e.connections.len());
            }
            if p.state != State::Running {
                break p;
            }
            assert!(Instant::now() < end, "still running: {:?}", log_of(&p));
            std::thread::sleep(Duration::from_millis(50));
        };
        let log = log_of(&p);
        assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
        assert!(p.helper.is_none(), "{:?}", p.helper);
        // Named for what it holds (#569): TS pieces joined without ffmpeg are a `.ts`.
        let State::Done(file) = &p.state else {
            unreachable!()
        };
        let saved = std::fs::read(file).unwrap();
        let left: Vec<String> = std::fs::read_dir(out.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".mdm"))
            .collect();
        assert!(left.is_empty(), "left behind: {left:?}");
        (saved, log, most)
    };
    let hls = |playlist: &str, extra: &str| {
        format!(
            r#"{{"format_id": "hls-720p", "ext": "mp4", "protocol": "m3u8_native",
              "vcodec": "avc1", "acodec": "none", "url": "{}"{extra}}}"#,
            lab.url(playlist)
        )
    };
    let plain: Vec<u8> = (0..3)
        .flat_map(|i| get(&format!("/hls/720p/seg{i}.ts")))
        .chain(audio.iter().copied())
        .collect();

    // HLS: three pieces, two at once, joined in order.
    let (saved, log, most) = run(hls("/hls/720p/index.m3u8", ""), 2);
    assert!(saved == plain, "HLS pieces joined in order");
    assert!(most >= 2, "pieces at once: {most}");
    assert!(
        log.iter()
            .any(|t| t.starts_with("Format hls-720p: 3 pieces, 2 at once")),
        "{log:?}"
    );

    // AES-128: the playlist's key, the first piece's own IV and the others' sequence numbers.
    let (saved, log, _) = run(hls("/hls/720p/enc.m3u8", ""), 3);
    assert!(saved == plain, "decrypted as the plain pieces: {log:?}");

    // DASH: yt-dlp's list (an init section and three pieces, from its base address).
    let dash = format!(
        r#"{{"format_id": "dash-720p", "ext": "mp4", "protocol": "http_dash_segments",
          "vcodec": "avc1", "acodec": "none", "url": "{}",
          "fragment_base_url": "{}",
          "fragments": [{{"path": "init.mp4"}}, {{"path": "seg1.m4s"}}, {{"path": "seg2.m4s"}},
                        {{"url": "{}"}}]}}"#,
        lab.url("/dash/manifest.mpd"),
        lab.url("/dash/720p/"),
        lab.url("/dash/720p/seg3.m4s")
    );
    let want: Vec<u8> = ["init.mp4", "seg1.m4s", "seg2.m4s", "seg3.m4s"]
        .iter()
        .flat_map(|f| get(&format!("/dash/720p/{f}")))
        .chain(audio.iter().copied())
        .collect();
    let (saved, _, _) = run(dash, 4);
    assert!(saved == want, "DASH pieces joined in order");

    // Pieces whose addresses expire together, 1.5 s after the first piece of each look-up's list
    // is asked, one at a time at 256 KB/s (a second each): yt-dlp is asked for the list again,
    // and the pieces go on from the new one. Timed from the first request, as above.
    let pieces: Vec<String> = (1..=4)
        .map(|i| {
            format!(
                r#"{{"url": "{}"}}"#,
                lab.url(&format!(
                    "/gen/256K/p{i}?seed={}&rate=256K&lasts_ms=1500&link={{LINK}}",
                    20 + i
                ))
            )
        })
        .collect();
    let expiring = format!(
        r#"{{"format_id": "dash-exp", "ext": "mp4", "protocol": "http_dash_segments",
          "vcodec": "avc1", "acodec": "none", "url": "{}", "fragments": [{}]}}"#,
        lab.url("/dash/manifest.mpd"),
        pieces.join(", ")
    );
    let want: Vec<u8> = (1..=4u64)
        .flat_map(|i| {
            let mut b = vec![0; 256 * 1024];
            mdm_labserver::content::fill(20 + i, 0, &mut b);
            b
        })
        .chain(audio.iter().copied())
        .collect();
    let (saved, log, _) = run(expiring, 1);
    let first = saved.iter().zip(&want).position(|(a, b)| a != b);
    assert!(
        saved == want,
        "whole across new lists: {} vs {} bytes, first difference at {first:?}: {log:?}",
        saved.len(),
        want.len()
    );
    assert!(
        log.iter()
            .any(|t| t == "Format dash-exp: Asking yt-dlp for the pieces' new addresses"),
        "{log:?}"
    );
}

/// #686: YouTube refusing the signed-in viewer. The browser's extension sends the page's cookies;
/// a look-up with them gets the signed-in viewer's page, whose stream the lab refuses (403, as
/// googlevideo.com refuses the reporter's), where the page without them downloads (a private
/// window). The download tries once more without the cookies and every run after that goes
/// without them; a video only signed-in viewers may watch, and another site, keep them and fail
/// as before. A look-up YouTube's bot check refuses with them is made without them; a members'
/// video's isn't.
fn signed_in_checks(
    me: &Path,
    lab: &mdm_labserver::LabServer,
    engine: &dyn Fn(u32) -> mdm_tools::streams::Engine,
) {
    use mdm_tools::resolve::{Looked, look_up};
    let page = |video: &str, availability: &str| {
        format!(
            r#"{{"id": "jNQXAC9IVRw", "title": "Talk", "ext": "mp4", "availability": "{availability}",
            "formats": [{{"format_id": "18", "ext": "mp4", "protocol": "https", "filesize": {},
              "vcodec": "avc1", "acodec": "mp4a", "url": "{}"}}]}}"#,
            1 << 20,
            lab.url(video)
        )
    };
    let browser = mdm_tools::proc::FromBrowser {
        cookies: Some("SAPISID=made-up; LOGIN_INFO=made-up".into()),
        ..Default::default()
    };
    let youtube = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
    // `availability` of the video on both pages; the end state, each run's cookies (`J with`,
    // `O without`…) and what was saved.
    let run = |url: &str, availability: &str| {
        let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(
            tools.path().join("page.json"),
            page("/gen/1M/v.mp4?seed=5", availability),
        )
        .unwrap();
        std::fs::write(
            tools.path().join("page-signed-in.json"),
            page("/gen/1M/v.mp4?seed=5&refuse=403", availability),
        )
        .unwrap();
        let exe = fake_in(me, tools.path());
        let job = VideoJob::start(
            &exe,
            url,
            "18",
            out.path(),
            "Talk",
            None,
            None,
            None,
            &browser,
            &Subs::default(),
            Some(engine(2)),
        )
        .expect("started");
        let end = Instant::now() + Duration::from_secs(120);
        let p = loop {
            let p = job.progress();
            if p.state != State::Running {
                break p;
            }
            assert!(Instant::now() < end, "still running: {:?}", log_of(&p));
            std::thread::sleep(Duration::from_millis(50));
        };
        let runs: Vec<String> = std::fs::read_to_string(tools.path().join("cookies.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        let saved = match &p.state {
            State::Done(f) => std::fs::read(f).ok(),
            _ => None,
        };
        (p, runs, saved)
    };
    let mut want = vec![0u8; 1 << 20];
    mdm_labserver::content::fill(5, 0, &mut want);

    // A public YouTube video: refused with the cookies, downloaded without them, as a private
    // window gets it; every run after the refusal goes without them.
    let (p, runs, saved) = run(youtube, "public");
    let log = log_of(&p);
    println!("fake_ytdlp: #686 public: {:?} {runs:?} {log:?}", p.state);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert!(
        saved.as_deref() == Some(&want[..]),
        "the private window's stream"
    );
    assert!(
        log.iter().any(|t| t.contains(
            "YouTube refused the video with the browser's cookies: looking it up again without them"
        )),
        "{log:?}"
    );
    assert_eq!(
        log.last().map(String::as_str),
        Some("Downloaded without the browser's cookies"),
        "which way worked: {log:?}"
    );
    assert_eq!(runs[0], "J with", "the cookies first: {runs:?}");
    let first_without = runs.iter().position(|r| r.ends_with("without")).unwrap();
    assert_eq!(runs[first_without], "J without", "{runs:?}");
    assert!(
        runs[first_without..].iter().all(|r| r.ends_with("without")),
        "never the cookies again: {runs:?}"
    );
    assert_eq!(
        runs.iter().filter(|r| *r == "J without").count(),
        1,
        "one look-up without them: {runs:?}"
    );

    // A channel members' video (yt-dlp: only signed-in viewers may watch it): the cookies stay,
    // and it fails as before.
    let (p, runs, _) = run(youtube, "subscriber_only");
    println!("fake_ytdlp: #686 members: {:?} {runs:?}", p.state);
    assert!(
        matches!(&p.state, State::Failed(e) if e.contains("403")),
        "{:?}",
        p.state
    );
    assert!(runs.iter().all(|r| r.ends_with(" with")), "{runs:?}");

    // The download's own look-up refused by YouTube's bot check with the cookies: made without
    // them, and the rest of that video's download goes without them too.
    let (p, runs, saved) = run("https://www.youtube.com/watch?v=botcheck000", "public");
    let log = log_of(&p);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert!(saved.as_deref() == Some(&want[..]));
    assert_eq!(runs, ["J with", "J without", "O without", "D without"]);
    assert_eq!(
        log.last().map(String::as_str),
        Some("Downloaded without the browser's cookies"),
        "{log:?}"
    );

    // Another site: as before.
    let (p, runs, _) = run("https://video.example/talk", "public");
    assert!(matches!(&p.state, State::Failed(_)), "{:?}", p.state);
    assert!(runs.iter().all(|r| r.ends_with(" with")), "{runs:?}");

    // The look-up itself refused by YouTube's bot check with the cookies: made once more without
    // them, and said so.
    let tools = tempfile::tempdir().unwrap();
    let exe = fake_in(me, tools.path());
    let bot = "https://www.youtube.com/watch?v=botcheck000";
    let (args, _file) = browser.args(bot).unwrap();
    let ours: Vec<String> = ["-J".to_owned()].into_iter().chain(args).collect();
    let out = mdm_tools::proc::look_up(&exe, &ours, bot, Duration::from_secs(30)).unwrap();
    assert!(out.without_cookies, "the second look-up, without them");
    assert!(matches!(
        look_up(&exe, bot, &browser).unwrap(),
        Looked::Video(_)
    ));
    // A members' video: refused for want of the sign-in, so not made again without it.
    let members = "https://www.youtube.com/watch?v=members0000";
    std::fs::remove_file(tools.path().join("cookies.log")).unwrap();
    let e = look_up(&exe, members, &browser).unwrap_err().to_string();
    assert!(e.contains("Join this channel"), "{e}");
    let runs = std::fs::read_to_string(tools.path().join("cookies.log")).unwrap();
    assert_eq!(runs.lines().collect::<Vec<_>>(), ["J with"], "tried once");
    println!("fake_ytdlp: #686 checks ok");
}

/// #686, live, only with `MDM_LIVE_YTDLP=<yt-dlp>` (and `MDM_LIVE_PAGE=<a public YouTube video>`,
/// a short one: its 360p format 18 is downloaded, then deleted):
///
///   MDM_LIVE_YTDLP=…\yt-dlp.exe MDM_LIVE_PAGE=https://www.youtube.com/watch?v=jNQXAC9IVRw cargo test -p mdm-tools --test fake_ytdlp
///
/// The real yt-dlp looks the video up without cookies, as MDM does (no account, signed in
/// nowhere). The fake then stands in for it with that answer: as is for a look-up without the
/// browser's cookies, and with every stream's address expired for one with them, which
/// googlevideo.com really refuses with 403, as it refuses a signed-in viewer's. The made-up
/// cookies go to the fake only. MDM's engine downloads the video from YouTube's servers once the
/// download has gone without the cookies.
fn live_checks(me: &Path) {
    let Some(real) = std::env::var_os("MDM_LIVE_YTDLP").map(PathBuf::from) else {
        return;
    };
    let page = std::env::var("MDM_LIVE_PAGE")
        .unwrap_or("https://www.youtube.com/watch?v=jNQXAC9IVRw".into());
    let t = Instant::now();
    let (looked, out) =
        mdm_tools::resolve::look_up_kept(&real, &page, &Default::default()).expect("looked up");
    assert!(!out.without_cookies);
    let mdm_tools::resolve::Looked::Video(v) = looked else {
        panic!("a list")
    };
    println!(
        "fake_ytdlp: live: looked {page} up without cookies in {:?}: {:?}, {} formats",
        t.elapsed(),
        v.title,
        v.formats.len()
    );
    let mut info: serde_json::Value = serde_json::from_slice(&out.json).unwrap();
    assert_eq!(info["availability"], "public", "a public video");
    // yt-dlp's own choice of formats goes: the fake chooses as asked (format 18).
    if let Some(o) = info.as_object_mut() {
        o.remove("requested_formats");
    }
    let tools = tempfile::tempdir().unwrap();
    std::fs::write(tools.path().join("page.json"), info.to_string()).unwrap();
    // Every googlevideo.com address, expired.
    fn expired(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::String(s) if s.contains(".googlevideo.com/") => {
                if let Ok(mut u) = url::Url::parse(s) {
                    let pairs: Vec<(String, String)> = u
                        .query_pairs()
                        .map(|(k, v)| {
                            let v = if k == "expire" {
                                "1600000000".into()
                            } else {
                                v.into_owned()
                            };
                            (k.into_owned(), v)
                        })
                        .collect();
                    u.query_pairs_mut().clear().extend_pairs(pairs);
                    *s = u.to_string();
                }
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(expired),
            serde_json::Value::Object(o) => o.values_mut().for_each(expired),
            _ => {}
        }
    }
    expired(&mut info);
    std::fs::write(tools.path().join("page-signed-in.json"), info.to_string()).unwrap();
    let exe = fake_in(me, tools.path());
    let out = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let browser = mdm_tools::proc::FromBrowser {
        cookies: Some("SAPISID=made-up; LOGIN_INFO=made-up".into()),
        ..Default::default()
    };
    let job = VideoJob::start(
        &exe,
        &page,
        "18",
        out.path(),
        "Live",
        None,
        None,
        None,
        &browser,
        &Subs::default(),
        Some(mdm_tools::streams::Engine {
            handle: rt.handle().clone(),
            opts: Default::default(),
        }),
    )
    .expect("started");
    let end = Instant::now() + Duration::from_secs(180);
    let p = loop {
        let p = job.progress();
        if p.state != State::Running {
            break p;
        }
        assert!(Instant::now() < end, "still running: {:?}", log_of(&p));
        std::thread::sleep(Duration::from_millis(100));
    };
    let log = log_of(&p);
    let runs = std::fs::read_to_string(tools.path().join("cookies.log")).unwrap_or_default();
    println!("fake_ytdlp: live: {:?}\n  runs: {runs:?}", p.state);
    for line in &log {
        println!("  log: {line}");
    }
    let State::Done(file) = &p.state else {
        panic!("{:?}: {log:?}", p.state)
    };
    let size = std::fs::metadata(file).unwrap().len();
    println!("fake_ytdlp: live: saved {size} bytes from YouTube's servers, then deleted");
    assert!(size > 0 && size < 8 << 20, "{size}");
    assert!(log.iter().any(|t| t.contains("403")), "{log:?}");
    assert_eq!(
        log.last().map(String::as_str),
        Some("Downloaded without the browser's cookies")
    );
}
