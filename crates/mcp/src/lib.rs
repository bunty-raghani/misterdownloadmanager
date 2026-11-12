//! Mister Download Manager's MCP server (Model Context Protocol). An AI assistant (Claude, Cursor,
//! VS Code, …) starts it and, through it, lists the running app's downloads, adds new ones and
//! pauses, resumes or removes them. A thin bridge to the app's automation API
//! (docs/command-line.md): nothing here downloads by itself. Guide: docs/ai-assistants.md.

use std::sync::Arc;

use mdm_ipc::Command;
use mdm_ipc::api::{API_OFF as API_OFF_CODE, DownloadInfo, ErrorAnswer, ListAnswer, STATUSES};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

/// How a tool reaches the app: [`mdm_ipc::request`] in the program, a stand-in in tests.
pub type Ask = Arc<dyn Fn(&Command) -> Result<String, String> + Send + Sync>;

/// Downloads listed when the assistant doesn't say how many.
const DEFAULT_LIMIT: usize = 100;

const NOT_RUNNING: &str = "Mister Download Manager isn't running. Ask the user to start it, then \
                           try again.";
const API_OFF: &str = "Mister Download Manager's automation API is off. Ask the user to turn it \
                       on once with `misterdownloadmanager-cli api on` (it stays on), then try \
                       again.";

const INSTRUCTIONS: &str = "Controls Mister Download Manager, the download manager running on \
    this computer. list_downloads shows its downloads with their ids; pause_downloads, \
    resume_downloads and remove_downloads act on those ids; add_download adds an address \
    (http, https, ftp, ftps or a magnet link); start_queue starts the main download queue. \
    Listing and acting on downloads needs the app's automation API, which the user turns on \
    with `misterdownloadmanager-cli api on`. Sizes and speeds are in bytes and bytes per second.";

#[derive(Deserialize, JsonSchema, Default)]
pub struct ListArgs {
    /// Only downloads with this status: Queued, Downloading, Stopped, Complete or Error.
    pub status: Option<String>,
    /// Only downloads whose file name or address contains this text (any case).
    pub search: Option<String>,
    /// At most this many, newest first (default 100).
    pub limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AddArgs {
    /// The address: http, https, ftp or ftps, or a magnet link (the app then asks the user
    /// which of the torrent's files to take).
    pub url: String,
    /// The folder to save it in (default: the app's folder for that kind of file).
    pub folder: Option<String>,
    /// The file name to save it as (default: from the address or the server).
    pub file_name: Option<String>,
    /// true: only add it to the main queue, without starting it now.
    #[serde(default)]
    pub later: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct IdsArgs {
    /// Download ids, from list_downloads.
    pub ids: Vec<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RemoveArgs {
    /// Download ids, from list_downloads. A running download must be paused first.
    pub ids: Vec<u64>,
    /// true: delete the downloaded files from the disk too; false (the default): keep them.
    #[serde(default)]
    pub delete_files: bool,
}

#[derive(Clone)]
pub struct Server {
    ask: Ask,
    tool_router: ToolRouter<Self>,
}

impl Server {
    pub fn new(ask: Ask) -> Self {
        Self {
            ask,
            tool_router: Self::tool_router(),
        }
    }

    /// The app's answer, or what went wrong in words the assistant can pass on. The channel
    /// blocks (up to a few seconds), so it runs off the async threads.
    async fn call(&self, cmd: Command) -> Result<String, String> {
        let ask = self.ask.clone();
        let answer = tokio::task::spawn_blocking(move || ask(&cmd))
            .await
            .map_err(|e| e.to_string())?;
        explain(answer)
    }

    async fn act(&self, action: &str, ids: Vec<u64>) -> Result<String, String> {
        if ids.is_empty() {
            return Err("No download ids given (list_downloads shows them).".into());
        }
        self.call(Command {
            action: Some(action.into()),
            ids,
            ..Default::default()
        })
        .await
    }
}

#[tool_router]
impl Server {
    #[tool(
        description = "Lists Mister Download Manager's downloads, newest first: id, file, url, \
                       path, status (Queued, Downloading, Stopped, Complete or Error), size and \
                       downloaded (bytes; size null while unknown), speed (bytes per second), \
                       when it was added and, for a failed one, why (error); with the total \
                       speed and how many match.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_downloads(
        &self,
        Parameters(args): Parameters<ListArgs>,
    ) -> Result<String, String> {
        let answer = self
            .call(Command {
                query: Some("list".into()),
                ..Default::default()
            })
            .await?;
        filter_list(&answer, &args)
    }

    #[tool(
        description = "Adds a download to Mister Download Manager and starts it (or only queues \
                       it, with later). If the list already has the address, the app asks the \
                       user what to do.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = true
        )
    )]
    async fn add_download(&self, Parameters(args): Parameters<AddArgs>) -> Result<String, String> {
        let url = args.url.trim().to_owned();
        if !addable(&url) {
            return Err(format!(
                "{url:?} isn't an address Mister Download Manager downloads: it takes http, \
                 https, ftp and ftps addresses and magnet links."
            ));
        }
        self.call(Command {
            url: Some(url.clone()),
            dir: args.folder.filter(|d| !d.trim().is_empty()),
            name: args.file_name.filter(|n| !n.trim().is_empty()),
            later: args.later,
            silent: true,
            ..Default::default()
        })
        .await?;
        Ok(if args.later {
            format!("Added {url} to the main queue (not started). list_downloads shows it.")
        } else {
            format!("Added {url} and started it. list_downloads shows its progress.")
        })
    }

    #[tool(
        description = "Pauses running downloads by id. Answers {\"done\": [ids], \"skipped\": \
                       [{\"id\", \"why\"}]}.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn pause_downloads(
        &self,
        Parameters(args): Parameters<IdsArgs>,
    ) -> Result<String, String> {
        self.act("pause", args.ids).await
    }

    #[tool(
        description = "Starts or resumes stopped, queued or failed downloads by id. Answers \
                       {\"done\": [ids], \"skipped\": [{\"id\", \"why\"}]}.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn resume_downloads(
        &self,
        Parameters(args): Parameters<IdsArgs>,
    ) -> Result<String, String> {
        self.act("start", args.ids).await
    }

    #[tool(
        description = "Removes downloads from Mister Download Manager's list by id, keeping \
                       their files, or deleting them from the disk too with delete_files. A \
                       running download must be paused first. Answers {\"done\": [ids], \
                       \"skipped\": [{\"id\", \"why\"}]}.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true
        )
    )]
    async fn remove_downloads(
        &self,
        Parameters(args): Parameters<RemoveArgs>,
    ) -> Result<String, String> {
        let action = if args.delete_files {
            "delete"
        } else {
            "remove"
        };
        self.act(action, args.ids).await
    }

    #[tool(
        description = "Starts Mister Download Manager's main download queue.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn start_queue(&self) -> Result<String, String> {
        self.call(Command {
            start_queue: true,
            ..Default::default()
        })
        .await?;
        Ok("Started the main download queue.".into())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::new(
            "misterdownloadmanager",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(INSTRUCTIONS)
    }
}

/// The app's answer read: a channel failure, a timeout, a refusal or an `{"error"}` become the
/// tool's error.
fn explain(answer: Result<String, String>) -> Result<String, String> {
    let answer = answer.map_err(|e| format!("{NOT_RUNNING} ({e})"))?;
    match answer.as_str() {
        "timeout" => {
            return Err(
                "Mister Download Manager didn't answer in time (a dialog may be open \
                        in it); try again."
                    .into(),
            );
        }
        "denied" => {
            return Err(
                "Mister Download Manager refused the request: its channel file \
                        (ipc.json) is from another start. Ask the user to restart the app."
                    .into(),
            );
        }
        "gone" => {
            return Err(
                "Mister Download Manager is closing; try again once it has started.".into(),
            );
        }
        _ => {}
    }
    if let Ok(e) = serde_json::from_str::<ErrorAnswer>(&answer) {
        return Err(if e.code.as_deref() == Some(API_OFF_CODE) {
            API_OFF.into()
        } else {
            format!("Mister Download Manager: {}", e.error)
        });
    }
    Ok(answer)
}

/// What the app adds (`mdm_engine::supported`, and magnet links for Add torrent).
fn addable(url: &str) -> bool {
    if url
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("magnet:"))
    {
        return true;
    }
    url::Url::parse(url)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https" | "ftp" | "ftps") && u.has_host())
}

/// The list answer cut to what was asked for, newest (highest id) first, with how many match.
fn filter_list(answer: &str, args: &ListArgs) -> Result<String, String> {
    let list: ListAnswer =
        serde_json::from_str(answer).map_err(|e| format!("unreadable answer: {e}"))?;
    let status = match args
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(s) => Some(
            STATUSES
                .into_iter()
                .find(|k| k.eq_ignore_ascii_case(s))
                .ok_or_else(|| format!("Unknown status {s:?}: use one of {STATUSES:?}."))?,
        ),
    };
    let search = args
        .search
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let mut downloads: Vec<DownloadInfo> = list
        .downloads
        .into_iter()
        .filter(|d| status.is_none_or(|s| d.status == s))
        .filter(|d| {
            search.as_ref().is_none_or(|q| {
                d.file.to_lowercase().contains(q) || d.url.to_lowercase().contains(q)
            })
        })
        .collect();
    let matching = downloads.len();
    downloads.sort_by_key(|d| std::cmp::Reverse(d.id));
    downloads.truncate(args.limit.unwrap_or(DEFAULT_LIMIT));
    Ok(serde_json::json!({
        "downloads": downloads,
        "matching": matching,
        "speed": list.speed,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const LIST: &str = r#"{"downloads":[
        {"id":1,"file":"ubuntu.iso","url":"https://releases.ubuntu.com/ubuntu.iso","status":"Complete","path":"/downloads/f","size":100,"downloaded":50,"speed":0,"added":null},
        {"id":2,"file":"7z.exe","url":"https://www.7-zip.org/a/7z.exe","status":"Downloading","path":"/downloads/f","size":100,"downloaded":50,"speed":0,"added":null},
        {"id":3,"file":"report.pdf","url":"https://example.com/report.pdf","status":"Stopped","path":"/downloads/f","size":100,"downloaded":50,"speed":0,"added":null}
    ],"speed":1234}"#;

    fn ids(answer: &str) -> Vec<u64> {
        let v: Value = serde_json::from_str(answer).unwrap();
        v["downloads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_u64().unwrap())
            .collect()
    }

    #[test]
    fn lists_newest_first_filtered_and_cut() {
        let all = filter_list(LIST, &ListArgs::default()).unwrap();
        assert_eq!(ids(&all), [3, 2, 1]);
        assert!(all.contains(r#""matching":3"#) && all.contains(r#""speed":1234"#));
        let args = ListArgs {
            status: Some("downloading".into()),
            ..Default::default()
        };
        assert_eq!(ids(&filter_list(LIST, &args).unwrap()), [2]);
        let args = ListArgs {
            search: Some("UBUNTU".into()),
            ..Default::default()
        };
        assert_eq!(ids(&filter_list(LIST, &args).unwrap()), [1]);
        let args = ListArgs {
            limit: Some(1),
            ..Default::default()
        };
        let one = filter_list(LIST, &args).unwrap();
        assert_eq!(ids(&one), [3]);
        assert!(one.contains(r#""matching":3"#));
        let args = ListArgs {
            status: Some("paused".into()),
            ..Default::default()
        };
        assert!(
            filter_list(LIST, &args)
                .unwrap_err()
                .contains("Unknown status")
        );
    }

    #[test]
    fn explains_what_went_wrong() {
        assert!(
            explain(Err("refused".into()))
                .unwrap_err()
                .starts_with(NOT_RUNNING)
        );
        assert!(
            explain(Ok("timeout".into()))
                .unwrap_err()
                .contains("in time")
        );
        assert!(
            explain(Ok("denied".into()))
                .unwrap_err()
                .contains("restart")
        );
        assert!(explain(Ok("gone".into())).unwrap_err().contains("closing"));
        let off = r#"{"error":"the automation API is off","code":"api-off"}"#;
        assert_eq!(explain(Ok(off.into())).unwrap_err(), API_OFF);
        let other = r#"{"error":"unknown action \"x\""}"#;
        assert!(
            explain(Ok(other.into()))
                .unwrap_err()
                .contains("unknown action")
        );
        assert_eq!(explain(Ok("ok".into())).unwrap(), "ok");
    }

    #[test]
    fn adds_only_what_the_app_downloads() {
        assert!(addable("https://example.com/a.zip"));
        assert!(addable("ftp://example.com/a.zip"));
        assert!(addable("MAGNET:?xt=urn:btih:abc"));
        assert!(!addable("file:///etc/passwd"));
        assert!(!addable("javascript:alert(1)"));
        assert!(!addable("example.com/a.zip"));
    }
}
