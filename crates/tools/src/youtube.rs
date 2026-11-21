//! YouTube with the browser's cookies (#686). The browser's extension sends MDM the cookies of
//! the page a video is on, so a private or a members' video downloads as it plays there. Signed
//! in, yt-dlp asks YouTube as the signed-in players do, and YouTube refuses those more often than
//! a visitor's: it wants proofs from them (PO tokens) that yt-dlp can't give, and answers the
//! look-up with its bot check, or the streams with 403. A private window, without the cookies,
//! got the same video. These tell such a refusal from a video that needs the sign-in.
//!
//! yt-dlp's words for each are in `tests/fixtures/ytdlp-refusals`.

use serde_json::Value;

/// Is `url` a page of YouTube's (youtube.com and its hosts, youtu.be, youtube-nocookie.com)?
pub fn is_youtube(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = u.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    ["youtube.com", "youtube-nocookie.com", "youtu.be"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// Does yt-dlp's message say the video needs a sign-in: a private video, an age-restricted one,
/// a channel members' or a paid one? The cookies are what it needs then.
pub fn needs_sign_in(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    [
        "private video",
        "confirm your age",
        "members-only",
        "channel's members",
        "join this channel",
        "premium members",
        "requires payment",
    ]
    .iter()
    .any(|k| m.contains(k))
}

/// Did YouTube refuse what was asked with the browser's cookies in a way a private window gets
/// past (yt-dlp's message, or MDM's engine's for a stream)?
/// - its bot check, "Sign in to confirm you're not a bot", which a signed-in session gets when
///   YouTube no longer takes its cookies;
/// - 403 from its servers: the player API's at the look-up ("Unable to download API page: HTTP
///   Error 403"), googlevideo.com's for a stream (yt-dlp's "unable to download video data: HTTP
///   Error 403: Forbidden", the engine's "server answered 403 Forbidden", a piece's "the server
///   answered 403 Forbidden"), or a stream's address the engine asked yt-dlp for again and that
///   was refused all the same ("the download link has expired");
/// - a PO token a signed-in player has to give.
///
/// Never a video that needs the sign-in ([`needs_sign_in`]).
pub fn refused_signed_in(message: &str) -> bool {
    if needs_sign_in(message) {
        return false;
    }
    let m = message.to_ascii_lowercase();
    [
        "not a bot",
        "http error 403",
        "answered 403",
        "link has expired",
        "po token",
    ]
    .iter()
    .any(|k| m.contains(k))
}

/// Does yt-dlp's answer for a video say only a signed-in viewer may watch it (its
/// `availability`: private, a channel members', Premium's, or behind a sign-in such as an age
/// check)? Its cookies stay then, whatever its streams answer.
pub fn signed_in_only(info: &Value) -> bool {
    matches!(
        info.get("availability").and_then(Value::as_str),
        Some("private" | "subscriber_only" | "premium_only" | "needs_auth")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// yt-dlp's `ERROR:` line in a fixture, as `proc::Spawned::failure` gives it.
    fn error(stderr: &str) -> String {
        stderr
            .lines()
            .rev()
            .find_map(|l| l.strip_prefix("ERROR:"))
            .expect("an ERROR: line")
            .trim()
            .to_owned()
    }

    /// #686: which of yt-dlp's real failures with the browser's cookies are tried again without
    /// them (`tests/fixtures/ytdlp-refusals`, its stderr as it wrote it).
    #[test]
    fn youtubes_refusals_of_the_signed_in_path_are_told_from_a_videos_sign_in() {
        let fixture = |name: &str| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/ytdlp-refusals")
                .join(name);
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        };
        // Refused as a signed-in viewer: a private window gets past these.
        for name in ["stream-403.stderr", "bot-check.stderr", "api-403.stderr"] {
            let e = error(&fixture(name));
            assert!(refused_signed_in(&e), "{name}: {e}");
            assert!(!needs_sign_in(&e), "{name}: {e}");
        }
        // The very words of #686.
        assert!(refused_signed_in(
            "yt-dlp failed: unable to download video data: HTTP Error 403: Forbidden"
        ));
        // The video needs the sign-in: the cookies stay, even where YouTube says "sign in".
        for name in ["private.stderr", "age.stderr", "members.stderr"] {
            let e = error(&fixture(name));
            assert!(needs_sign_in(&e), "{name}: {e}");
            assert!(!refused_signed_in(&e), "{name}: {e}");
        }
        // MDM's engine's words for a stream googlevideo.com refused (`mdm_engine::Error`), a
        // piece's (`pieces.rs`), and an address refused again after yt-dlp gave a new one.
        for e in [
            "server answered 403 Forbidden",
            "the playlist: the server answered 403 Forbidden",
            "piece 3: the server answered 403 Forbidden",
            "the download link has expired (refresh the download address)",
        ] {
            assert!(refused_signed_in(e), "{e}");
        }
        // A PO token yt-dlp couldn't give (its own words, a warning it may also fail with).
        let po = fixture("../ytdlp-js-runtime/challenge-failed.stderr");
        let po = po.lines().find(|l| l.contains("PO Token")).unwrap();
        assert!(refused_signed_in(po), "{po}");
        // Anything else stays as it was: another error, a server's, the network's.
        for e in [
            "[youtube] x: Video unavailable",
            "[youtube] Cr381pDsSsA: This video has been removed for violating YouTube's Terms of Service",
            "unable to download video data: HTTP Error 500: Internal Server Error",
            "server answered 404 Not Found",
            "[youtube] x: Requested format is not available. Use --list-formats for a list of available formats",
            "HTTPSConnectionPool(host='rr1---sn-x.googlevideo.com', port=443): Read timed out.",
        ] {
            assert!(!refused_signed_in(e), "{e}");
        }
        // yt-dlp saying the cookies weren't taken is a warning: the look-up went on (a cookie
        // file of made-up values, the look-up worked).
        let rotated = fixture("cookies-rotated.stderr");
        assert!(rotated.lines().all(|l| l.starts_with("WARNING:")));
    }

    #[test]
    fn youtubes_pages_and_no_others() {
        for u in [
            "https://www.youtube.com/watch?v=jNQXAC9IVRw",
            "https://m.youtube.com/watch?v=jNQXAC9IVRw",
            "https://music.youtube.com/watch?v=jNQXAC9IVRw",
            "https://YouTube.com/shorts/jNQXAC9IVRw",
            "https://youtu.be/jNQXAC9IVRw?si=x",
            "https://www.youtube-nocookie.com/embed/jNQXAC9IVRw",
        ] {
            assert!(is_youtube(u), "{u}");
        }
        for u in [
            "https://vimeo.com/1",
            "https://notyoutube.com/watch?v=x",
            "https://youtube.com.example/watch?v=x",
            "https://rr1---sn-x.googlevideo.com/videoplayback",
            "not an address",
        ] {
            assert!(!is_youtube(u), "{u}");
        }
    }

    /// yt-dlp's `availability` for a video that needs a sign-in keeps the cookies; a public or
    /// unlisted one's, or none, doesn't.
    #[test]
    fn a_video_only_signed_in_viewers_may_watch() {
        for a in ["private", "subscriber_only", "premium_only", "needs_auth"] {
            assert!(
                signed_in_only(&serde_json::json!({ "availability": a })),
                "{a}"
            );
        }
        for a in [
            serde_json::json!({ "availability": "public" }),
            serde_json::json!({ "availability": "unlisted" }),
            serde_json::json!({ "availability": null }),
            serde_json::json!({}),
        ] {
            assert!(!signed_in_only(&a), "{a}");
        }
    }
}
