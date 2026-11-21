//! Releases on GitHub, from its API, or from github.com's own pages when the API refuses for its
//! rate limit (#1337).
//!
//! GitHub's API answers 60 requests an hour for each internet address without an account. On a
//! network many people share (a mobile carrier's or an internet provider's shared address, a
//! campus, an office), others use them up, and the API refuses MDM: "403 rate limit exceeded".
//! github.com's own pages aren't limited that way: `<repo>/releases/latest` redirects to the
//! latest release's page (`<repo>/releases/tag/<tag>`), and each of its files is at
//! `<repo>/releases/download/<tag>/<name>`. The API is asked first (it lists the files, with
//! their sizes and dates); the pages only when it refuses for its limit. Only where a file's name
//! and address come from changes: what is downloaded is checked as it is from the API's (signed
//! checksum lists, checksums).

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

use crate::Error;

/// GitHub's own pages.
pub const WEB: &str = "https://github.com";
/// GitHub's server of a repository's files as they are at a tag or a branch.
pub const RAW: &str = "https://raw.githubusercontent.com";

/// What a path segment keeps as it is (RFC 3986's unreserved characters).
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// How many redirects [`latest_tag`] follows before the release's page (a repository renamed
/// redirects to its new name first).
const HOPS: usize = 5;

/// Whether an answer of GitHub's API (its `status`, its `x-ratelimit-remaining` header, its body)
/// is its refusal for the rate limit: 403 or 429 with none of the hour's requests left, or saying
/// so in its body (also the secondary limit's "You have exceeded a secondary rate limit").
pub fn rate_limited(status: u16, remaining: Option<&str>, body: &str) -> bool {
    matches!(status, 403 | 429)
        && (remaining.is_some_and(|r| r.trim() == "0")
            || body.to_ascii_lowercase().contains("rate limit"))
}

/// The tag of the release page `location` (`…/releases/tag/<tag>`, an address or a path, as a
/// redirect's `Location` or the address a request ended at), decoded. None for any other page (a
/// repository without releases redirects to its `releases` page) and for a tag that couldn't be
/// a folder's name.
pub fn tag_from(location: &str) -> Option<String> {
    let path = location.split(['?', '#']).next()?;
    let (_, tag) = path.rsplit_once("/releases/tag/")?;
    let tag = percent_encoding::percent_decode_str(tag)
        .decode_utf8()
        .ok()?;
    let fine = !tag.is_empty()
        && tag != "."
        && tag != ".."
        && !tag.contains(['/', '\\', ':'])
        && !tag.chars().any(char::is_control);
    fine.then(|| tag.into_owned())
}

/// `<web>/<repo>/releases/latest` (`repo`: `owner/name`), which redirects to the latest
/// release's page.
pub fn latest_page(web: &str, repo: &str) -> String {
    format!("{}/{repo}/releases/latest", web.trim_end_matches('/'))
}

/// `<web>/<repo>/releases/tag/<tag>`: the release's page.
pub fn release_page(web: &str, repo: &str, tag: &str) -> String {
    format!(
        "{}/{repo}/releases/tag/{}",
        web.trim_end_matches('/'),
        utf8_percent_encode(tag, SEGMENT)
    )
}

/// `<web>/<repo>/releases/download/<tag>/<name>`: release file `name`'s address (GitHub sends it
/// on to where the file is kept).
pub fn download_url(web: &str, repo: &str, tag: &str, name: &str) -> String {
    format!(
        "{}/{repo}/releases/download/{}/{}",
        web.trim_end_matches('/'),
        utf8_percent_encode(tag, SEGMENT),
        utf8_percent_encode(name, SEGMENT)
    )
}

/// `<raw>/<repo>/<tag>/<path>`: a file of the repository as it is at `tag` (`raw`:
/// [`RAW`], GitHub's server of them).
pub fn file_at(raw: &str, repo: &str, tag: &str, path: &str) -> String {
    format!(
        "{}/{repo}/{}/{path}",
        raw.trim_end_matches('/'),
        utf8_percent_encode(tag, SEGMENT)
    )
}

/// The latest release's tag of `repo` on GitHub's pages at `web`: where `releases/latest`
/// redirects. `client` follows no redirect ([`mdm_engine::hop_client`], or a client built with
/// `redirect::Policy::none()`): each hop is read here, up to [`HOPS`], on `web`'s server only, so
/// the release's page itself is never downloaded. A client that follows them works too (the tag is
/// then read from where it ended).
pub async fn latest_tag(client: &reqwest::Client, web: &str, repo: &str) -> Result<String, Error> {
    let start = latest_page(web, repo);
    let mut at = url::Url::parse(&start).map_err(|e| Error::BadRelease(e.to_string()))?;
    for _ in 0..HOPS {
        let resp = client.get(at.clone()).send().await?;
        if resp.status().is_redirection() {
            let next = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|l| l.to_str().ok())
                .and_then(|l| at.join(l).ok())
                .ok_or_else(|| Error::BadRelease(format!("{at} redirects nowhere")))?;
            if next.host_str() != at.host_str() {
                return Err(Error::BadRelease(format!("{start} leads to {next}")));
            }
            if let Some(tag) = tag_from(next.as_str()) {
                return Ok(tag);
            }
            at = next;
            continue;
        }
        let resp = resp.error_for_status()?;
        return tag_from(resp.url().as_str())
            .ok_or_else(|| Error::BadRelease(format!("{start} names no release")));
    }
    Err(Error::BadRelease(format!("{start}: too many redirects")))
}

/// What GitHub's API answered: its body, or its refusal for the rate limit (the request's error,
/// as it would have been returned without a way around it).
pub enum Api {
    Answer(bytes::Bytes),
    RateLimited(Error),
}

/// `url` (an address of GitHub's API) asked with `client`. Any other refusal is an error.
pub async fn api_get(client: &reqwest::Client, url: &str) -> Result<Api, Error> {
    let resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?;
    let Err(refusal) = resp.error_for_status_ref() else {
        return Ok(Api::Answer(resp.bytes().await?));
    };
    let status = resp.status().as_u16();
    let remaining = resp
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = resp.text().await.unwrap_or_default();
    if rate_limited(status, remaining.as_deref(), &body) {
        tracing::warn!("{url}: GitHub's API refused the request ({status}) for its rate limit");
        return Ok(Api::RateLimited(Error::Http(refusal)));
    }
    Err(Error::Http(refusal))
}

/// A release: its tag and its files (name, download address).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Latest {
    pub tag: String,
    pub files: Vec<(String, String)>,
    /// GitHub's own digest of each file it has one of (`sha256:<hex>`), the API's only: none
    /// from the pages.
    pub digests: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    /// `sha256:<hex>` (GitHub's digest of the upload).
    digest: Option<String>,
}

/// The latest release of `repo` (`owner/name`) from GitHub's API at `api` (its
/// `releases/latest`), every file it lists; or, when the API refuses for its rate limit, from the
/// pages at `web`: its tag, and the files `names` at their download addresses (whether each is
/// there shows when it is downloaded). When the pages fail too, the API's refusal is the error
/// (the pages' failure is logged).
pub(crate) async fn latest_release(
    net: &mdm_engine::Options,
    api: &str,
    web: &str,
    repo: &str,
    names: &[&str],
) -> Result<Latest, Error> {
    let client = crate::client(net, api)?;
    let refusal = match api_get(&client, api).await? {
        Api::Answer(body) => {
            let r: ApiRelease =
                serde_json::from_slice(&body).map_err(|e| Error::BadRelease(e.to_string()))?;
            return Ok(Latest {
                tag: r.tag_name,
                digests: r
                    .assets
                    .iter()
                    .filter_map(|a| Some((a.name.clone(), a.digest.clone()?)))
                    .collect(),
                files: r
                    .assets
                    .into_iter()
                    .map(|a| (a.name, a.browser_download_url))
                    .collect(),
            });
        }
        Api::RateLimited(refusal) => refusal,
    };
    let page = latest_page(web, repo);
    tracing::info!("{repo}: looking for its latest release on {page} instead (#1337)");
    let found = async {
        let url = url::Url::parse(&page).map_err(|e| Error::BadRelease(e.to_string()))?;
        let hops = mdm_engine::hop_client(net, &url).map_err(|error| match error {
            mdm_engine::Error::Adapter(a) => Error::Adapter(a),
            error => Error::Download {
                host: url.host_str().unwrap_or_default().to_owned(),
                error,
            },
        })?;
        latest_tag(&hops, web, repo).await
    }
    .await;
    match found {
        Ok(tag) => {
            tracing::info!("{repo}: latest release {tag}, from {page}");
            let files = names
                .iter()
                .map(|n| ((*n).to_owned(), download_url(web, repo, &tag, n)))
                .collect();
            Ok(Latest {
                tag,
                files,
                digests: Vec::new(),
            })
        }
        Err(e) => {
            tracing::warn!("{page}: {e}");
            Err(refusal)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_limit_is_told_from_other_refusals() {
        // GitHub's primary limit: no request left this hour.
        assert!(rate_limited(403, Some("0"), ""));
        assert!(rate_limited(429, Some(" 0 "), ""));
        // Its words, with or without the header (the secondary limit).
        assert!(rate_limited(
            403,
            None,
            r#"{"message":"API rate limit exceeded for 203.0.113.7."}"#
        ));
        assert!(rate_limited(
            403,
            Some("12"),
            "You have exceeded a secondary rate limit"
        ));
        // Any other refusal isn't: a blocked repository, a server error, a missing release.
        assert!(!rate_limited(
            403,
            Some("59"),
            r#"{"message":"Repository access blocked"}"#
        ));
        assert!(!rate_limited(403, None, ""));
        assert!(!rate_limited(404, Some("0"), "rate limit"));
        assert!(!rate_limited(500, Some("0"), "rate limit"));
    }

    #[test]
    fn the_tag_is_read_from_the_release_page_address() {
        // As github.com redirects today (checked 2026-11-17).
        assert_eq!(
            tag_from("https://github.com/anefzaoui/misterdownloadmanager/releases/tag/v0.5.2"),
            Some("v0.5.2".into())
        );
        assert_eq!(
            tag_from("https://github.com/yt-dlp/yt-dlp/releases/tag/2026.10.26"),
            Some("2026.10.26".into())
        );
        // A path, a query or an anchor, an encoded character.
        assert_eq!(
            tag_from("/x/y/releases/tag/v1.0.0-rc.1?a=b#c"),
            Some("v1.0.0-rc.1".into())
        );
        assert_eq!(tag_from("/x/y/releases/tag/v1%2B2"), Some("v1+2".into()));
        // No release yet (GitHub redirects to the list), or anything else.
        for none in [
            "https://github.com/x/y/releases",
            "https://github.com/x/y/releases/latest",
            "https://github.com/x/y/releases/tag/",
            "https://github.com/x/y/releases/tag/a/b",
            "https://github.com/x/y/releases/tag/..",
            "https://github.com/x/y/releases/tag/%2E%2E",
            "https://github.com/x/y/releases/tag/a%2Fb",
            "https://github.com/x/y/releases/tag/a%5Cb",
            "https://github.com/x/y/releases/tag/c%3A",
            "https://github.com/x/y/releases/tag/a%0Ab",
            "https://github.com/login",
            "",
        ] {
            assert_eq!(tag_from(none), None, "{none}");
        }
    }

    #[test]
    fn release_addresses_are_built_from_the_tag_and_the_name() {
        assert_eq!(
            latest_page(WEB, "yt-dlp/yt-dlp"),
            "https://github.com/yt-dlp/yt-dlp/releases/latest"
        );
        assert_eq!(
            release_page("http://127.0.0.1:9/gh/", "x/y", "v0.6.0"),
            "http://127.0.0.1:9/gh/x/y/releases/tag/v0.6.0"
        );
        assert_eq!(
            download_url(WEB, "yt-dlp/yt-dlp", "2026.10.26", "SHA2-256SUMS.sig"),
            "https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/SHA2-256SUMS.sig"
        );
        assert_eq!(
            download_url(
                WEB,
                "anefzaoui/misterdownloadmanager",
                "v0.6.0",
                "MisterDownloadManager-0.6.0-setup-arm64.exe"
            ),
            "https://github.com/anefzaoui/misterdownloadmanager/releases/download/v0.6.0/MisterDownloadManager-0.6.0-setup-arm64.exe"
        );
        assert_eq!(
            download_url(
                WEB,
                "BtbN/FFmpeg-Builds",
                "latest",
                "ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip"
            ),
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip"
        );
        // Whatever a name holds stays in its segment.
        assert_eq!(
            download_url(WEB, "x/y", "v1 2", "a/b?.zip"),
            "https://github.com/x/y/releases/download/v1%202/a%2Fb%3F.zip"
        );
        assert_eq!(
            file_at(RAW, "x/y", "v0.6.0", "CHANGELOG.md"),
            "https://raw.githubusercontent.com/x/y/v0.6.0/CHANGELOG.md"
        );
        // Built and read back, a tag is the same.
        for tag in ["v0.5.2", "2026.10.26.1", "v1+2", "release 1"] {
            assert_eq!(
                tag_from(&release_page(WEB, "x/y", tag)).as_deref(),
                Some(tag)
            );
        }
    }

    const MDM: &str = "anefzaoui/misterdownloadmanager";
    const ZIP: &str = "MisterDownloadManager-9.9.9-x64.zip";
    const SETUP: &str = "MisterDownloadManager-9.9.9-setup.exe";

    /// A release on the fake GitHub: its repository, its tag, its files as (name, fixture).
    type FakeRelease<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

    /// A lab server whose fake GitHub (`mdm_labserver`'s `github`) has `releases`; its API
    /// refuses every request for its rate limit.
    async fn fake_github(
        releases: &[FakeRelease<'_>],
    ) -> (mdm_labserver::LabServer, tempfile::TempDir) {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let dir = tempfile::tempdir().unwrap();
        for (repo, tag, files) in releases {
            let root = dir.path().join("gh").join(repo);
            std::fs::create_dir_all(root.join(tag)).unwrap();
            std::fs::write(root.join("LATEST"), format!("{tag}\n")).unwrap();
            for (name, fixture) in *files {
                std::fs::copy(fixtures.join(fixture), root.join(tag).join(name)).unwrap();
            }
        }
        let lab = mdm_labserver::LabServer::start(mdm_labserver::Config {
            files: Some(dir.path().to_owned()),
            ..Default::default()
        })
        .await
        .unwrap();
        (lab, dir)
    }

    /// #1337 end to end: GitHub's API refuses for its rate limit, so the latest release is found
    /// on the pages (the tag `releases/latest` redirects to), and its file is downloaded and
    /// checked as Update now checks it, against the release's list signed by MDM's key; a file
    /// that isn't the one the list names is still refused. yt-dlp's real signed list is fetched
    /// the same way and passes yt-dlp's key.
    #[tokio::test]
    async fn a_rate_limited_api_gives_way_to_the_release_pages() {
        let mdm_files: &[(&str, &str)] = &[
            ("SHA256SUMS.txt", "mdm-SHA256SUMS.txt"),
            ("SHA256SUMS.txt.sig", "mdm-SHA256SUMS.txt.sig"),
            (ZIP, ZIP),
            // Not what the signed list says it is.
            (SETUP, "mdm-SHA256SUMS.txt"),
        ];
        let ytdlp_files: &[(&str, &str)] = &[
            ("SHA2-256SUMS", "SHA2-256SUMS"),
            ("SHA2-256SUMS.sig", "SHA2-256SUMS.sig"),
        ];
        let (lab, _dir) = fake_github(&[
            (MDM, "v9.9.9", mdm_files),
            ("yt-dlp/yt-dlp", "2026.10.26", ytdlp_files),
        ])
        .await;
        let net = crate::net(mdm_engine::Proxy::None);
        let api = lab.url(&format!("/gh/api/repos/{MDM}/releases/latest"));
        let web = lab.url("/gh/web");
        let names = ["SHA256SUMS.txt", "SHA256SUMS.txt.sig", ZIP, SETUP];
        let found = latest_release(&net, &api, &web, MDM, &names).await.unwrap();
        assert_eq!(found.tag, "v9.9.9");
        assert_eq!(
            found.files[2],
            (
                ZIP.to_owned(),
                format!("{web}/{MDM}/releases/download/v9.9.9/{ZIP}")
            )
        );
        // The API first, then the release's page by its redirect, never the page itself.
        let paths: Vec<String> = lab.log().into_iter().map(|e| e.path).collect();
        assert_eq!(
            paths,
            [
                format!("/gh/api/repos/{MDM}/releases/latest"),
                format!("/gh/web/{MDM}/releases/latest"),
            ]
        );
        // Downloaded and checked as Update now does: the signed list, then the file.
        let dir = tempfile::tempdir().unwrap();
        let progress = crate::update::Progress::default();
        let file = crate::update::fetch(&found.files, ZIP, dir.path(), &net, &progress)
            .await
            .unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(ZIP);
        assert_eq!(
            std::fs::read(file).unwrap(),
            std::fs::read(fixture).unwrap()
        );
        let wrong = crate::update::fetch(&found.files, SETUP, dir.path(), &net, &progress).await;
        assert!(matches!(wrong, Err(Error::BadChecksum)), "{wrong:?}");
        // yt-dlp's release from its pages: its real list, signed by its key.
        let ytdlp = latest_release(
            &net,
            &lab.url("/gh/api/repos/yt-dlp/yt-dlp/releases/latest"),
            &web,
            "yt-dlp/yt-dlp",
            &crate::YTDLP_SUMS,
        )
        .await
        .unwrap();
        assert_eq!(ytdlp.tag, "2026.10.26");
        let url = |n: &str| ytdlp.files.iter().find(|f| f.0 == n).unwrap().1.clone();
        let client = crate::client(&net, &url("SHA2-256SUMS")).unwrap();
        let sums = crate::signed_list(
            &client,
            &url("SHA2-256SUMS"),
            &url("SHA2-256SUMS.sig"),
            crate::signed_by_ytdlp,
        )
        .await
        .unwrap();
        assert!(crate::sum_for(&sums, "yt-dlp_linux").is_some());
        // Signed by another key: refused, wherever it came from.
        let other = crate::signed_list(
            &client,
            &url("SHA2-256SUMS"),
            &url("SHA2-256SUMS.sig"),
            crate::signed_by_mdm,
        )
        .await;
        assert!(matches!(other, Err(Error::BadSignature)), "{other:?}");
    }

    /// When the pages don't give the release either (a repository without one here), the
    /// error is the API's refusal, which the app words as GitHub refusing too many requests from
    /// this network.
    #[tokio::test]
    async fn when_the_pages_fail_too_the_rate_limit_is_the_error() {
        let (lab, _dir) = fake_github(&[]).await;
        let net = crate::net(mdm_engine::Proxy::None);
        let api = lab.url("/gh/api/repos/x/y/releases/latest");
        let err = latest_release(&net, &api, &lab.url("/gh/web"), "x/y", &["a"])
            .await
            .unwrap_err();
        assert_eq!(
            err.network(),
            Some((
                "127.0.0.1".to_owned(),
                mdm_engine::fault::Fault::Status(403)
            )),
            "{err:?}"
        );
    }

    /// The API's own answer is taken when it gives one, every file it lists (the pages aren't
    /// asked); any other refusal than the rate limit is an error, without the pages.
    #[tokio::test]
    async fn the_api_is_asked_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("latest.json"),
            r#"{"tag_name":"v1.0.0","assets":[{"name":"a.zip","browser_download_url":"https://example.invalid/a.zip","digest":"sha256:00ff"},{"name":"b.zip","browser_download_url":"https://example.invalid/b.zip"}]}"#,
        )
        .unwrap();
        let lab = mdm_labserver::LabServer::start(mdm_labserver::Config {
            files: Some(dir.path().to_owned()),
            ..Default::default()
        })
        .await
        .unwrap();
        let net = crate::net(mdm_engine::Proxy::None);
        let web = lab.url("/gh/web");
        let found = latest_release(&net, &lab.url("/files/latest.json"), &web, "x/y", &["b"])
            .await
            .unwrap();
        assert_eq!(
            found,
            Latest {
                tag: "v1.0.0".into(),
                files: vec![
                    ("a.zip".into(), "https://example.invalid/a.zip".into()),
                    ("b.zip".into(), "https://example.invalid/b.zip".into())
                ],
                digests: vec![("a.zip".into(), "sha256:00ff".into())],
            }
        );
        let missing = latest_release(&net, &lab.url("/files/none.json"), &web, "x/y", &["b"]).await;
        assert!(
            matches!(&missing, Err(Error::Http(e)) if e.status().map(|s| s.as_u16()) == Some(404)),
            "{missing:?}"
        );
        assert!(
            lab.log().iter().all(|e| !e.path.starts_with("/gh/")),
            "the pages were asked"
        );
    }
}
