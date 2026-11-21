//! The MCP server end to end: the real program over stdio, as an assistant starts it, against a
//! stand-in for the app that listens and answers as the app's channel does (mdm_ipc).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// The app's stand-in: `ipc.json` in `dir`, one answer per connection, every request kept.
fn fake_app(dir: &std::path::Path, api_on: bool) -> Arc<Mutex<Vec<Value>>> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::fs::write(
        dir.join("ipc.json"),
        json!({ "port": port, "token": "secret" }).to_string(),
    )
    .unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let mut line = String::new();
            BufReader::new(&conn).read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            let answer = if req["token"] != "secret" {
                "denied".to_owned()
            } else if !api_on && (req["query"].is_string() || req["action"].is_string()) {
                json!({ "error": "the automation API is off", "code": "api-off" }).to_string()
            } else if req["query"] == "list" {
                json!({ "downloads": [
                    { "id": 1, "file": "old.zip", "url": "https://example.com/old.zip", "status": "Complete", "path": "/downloads/f", "size": 100, "downloaded": 50, "speed": 0, "added": null, "category": "Compressed" },
                    { "id": 2, "file": "new.iso", "url": "https://example.com/new.iso", "status": "Downloading", "path": "/downloads/f", "size": 100, "downloaded": 50, "speed": 0, "added": null, "category": "General" },
                ], "speed": 1000, "free": 20000000000u64,
                "queues": [
                    { "name": "Main", "running": true, "ids": [1] },
                    { "name": "Night", "running": false, "ids": [2] },
                ],
                "categories": [
                    { "key": "General", "name": "General" },
                    { "key": "Compressed", "name": "Compressed" },
                ] })
                .to_string()
            } else if req["action"].is_string() {
                json!({ "done": req["ids"], "skipped": [] }).to_string()
            } else {
                "ok".to_owned()
            };
            log.lock().unwrap().push(req);
            (&conn).write_all(format!("{answer}\n").as_bytes()).unwrap();
        }
    });
    seen
}

struct Mcp {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
}

impl Mcp {
    fn start(data: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_misterdownloadmanager-mcp"))
            .env("MDM_DATA_DIR", data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut mcp = Self {
            child,
            input,
            output,
            next: 1,
        };
        let init = mcp.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" },
            }),
        );
        assert_eq!(init["serverInfo"]["name"], "misterdownloadmanager");
        assert!(init["instructions"].as_str().unwrap().contains("api on"));
        mcp.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        mcp
    }

    fn send(&mut self, msg: Value) {
        writeln!(self.input, "{msg}").unwrap();
        self.input.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let mut line = String::new();
            assert!(
                self.output.read_line(&mut line).unwrap() > 0,
                "server closed"
            );
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == id {
                assert!(msg["error"].is_null(), "{msg}");
                return msg["result"].clone();
            }
        }
    }

    /// A tool's text and whether it is an error.
    fn call(&mut self, tool: &str, args: Value) -> (String, bool) {
        let r = self.request("tools/call", json!({ "name": tool, "arguments": args }));
        let text = r["content"][0]["text"].as_str().unwrap().to_owned();
        (text, r["isError"] == true)
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn an_assistant_lists_adds_and_acts_through_the_app() {
    let data = tempfile::tempdir().unwrap();
    let seen = fake_app(data.path(), true);
    let mut mcp = Mcp::start(data.path());

    let tools = mcp.request("tools/list", json!({}));
    let mut names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "add_download",
            "list_downloads",
            "pause_downloads",
            "remove_downloads",
            "resume_downloads",
            "start_queue",
            "stop_queue"
        ]
    );

    let (list, err) = mcp.call("list_downloads", json!({ "status": "downloading" }));
    assert!(!err, "{list}");
    let list: Value = serde_json::from_str(&list).unwrap();
    assert_eq!(list["downloads"][0]["id"], 2);
    assert_eq!(list["matching"], 1);
    assert_eq!(list["free"], 20000000000u64);
    assert!(list["queues"].is_array() && list["categories"].is_array());

    let (list, err) = mcp.call("list_downloads", json!({ "id": 1 }));
    assert!(!err, "{list}");
    let list: Value = serde_json::from_str(&list).unwrap();
    assert_eq!(list["downloads"][0]["id"], 1);
    assert_eq!(list["matching"], 1);

    let (list, err) = mcp.call("list_downloads", json!({ "category": "Compressed" }));
    assert!(!err, "{list}");
    let list: Value = serde_json::from_str(&list).unwrap();
    assert_eq!(list["downloads"][0]["id"], 1);
    assert_eq!(list["matching"], 1);

    let (list, err) = mcp.call("list_downloads", json!({ "queue": "Night" }));
    assert!(!err, "{list}");
    let list: Value = serde_json::from_str(&list).unwrap();
    assert_eq!(list["downloads"][0]["id"], 2);
    assert_eq!(list["matching"], 1);

    let (text, err) = mcp.call(
        "add_download",
        json!({ "url": "https://example.com/a.zip", "folder": "D:\\Files", "later": true }),
    );
    assert!(!err && text.contains("main queue"), "{text}");
    let (text, err) = mcp.call("add_download", json!({ "url": "file:///etc/passwd" }));
    assert!(err && text.contains("isn't an address"), "{text}");

    let (text, err) = mcp.call(
        "remove_downloads",
        json!({ "ids": [1], "delete_files": true }),
    );
    assert!(!err && text.contains(r#""done":[1]"#), "{text}");
    let (_, err) = mcp.call("pause_downloads", json!({ "ids": [2] }));
    assert!(!err);
    let (text, err) = mcp.call("start_queue", json!({}));
    assert!(!err && text.contains("main download queue"), "{text}");
    let (text, err) = mcp.call("stop_queue", json!({}));
    assert!(!err && text.contains("main download queue"), "{text}");

    let (text, err) = mcp.call(
        "add_download",
        json!({ "url": "https://example.com/b.zip", "later": true, "queue": "Night", "category": "Compressed", "referer": "https://example.com/page.html", "user_agent": "CustomAgent/1.0", "cookie": "session=xyz123; uid=42", "private": true }),
    );
    assert!(!err && text.contains(r#"queue "Night""#), "{text}");
    let (text, err) = mcp.call("start_queue", json!({ "queue": "Night" }));
    assert!(!err && text.contains(r#"queue "Night""#), "{text}");
    let (text, err) = mcp.call("stop_queue", json!({ "queue": "Night" }));
    assert!(!err && text.contains(r#"queue "Night""#), "{text}");

    // What reached the app: the fields its channel reads (mdm_ipc::Command), with the token.
    let seen = seen.lock().unwrap();
    let add = seen
        .iter()
        .find(|r| r["url"] == "https://example.com/a.zip")
        .unwrap();
    assert_eq!(add["url"], "https://example.com/a.zip");
    assert_eq!(add["dir"], "D:\\Files");
    assert_eq!(
        (add["later"].clone(), add["silent"].clone()),
        (json!(true), json!(true))
    );
    let add_night = seen
        .iter()
        .find(|r| r["url"] == "https://example.com/b.zip")
        .unwrap();
    assert_eq!(add_night["url"], "https://example.com/b.zip");
    assert_eq!(add_night["queue"], "Night");
    assert_eq!(add_night["category"], "Compressed");
    assert_eq!(add_night["referer"], "https://example.com/page.html");
    assert_eq!(add_night["user_agent"], "CustomAgent/1.0");
    assert_eq!(add_night["cookies"], "session=xyz123; uid=42");
    assert_eq!(add_night["private"], true);
    assert!(
        seen.iter()
            .any(|r| r["action"] == "delete" && r["ids"] == json!([1]))
    );
    assert!(
        seen.iter()
            .any(|r| r["action"] == "pause" && r["ids"] == json!([2]))
    );
    assert!(seen.iter().any(|r| r["start_queue"] == true));
    assert!(
        seen.iter()
            .any(|r| r["action"] == "stop_queue" && r["queue"].is_null())
    );
    assert!(
        seen.iter()
            .any(|r| r["action"] == "start_queue" && r["queue"] == "Night")
    );
    assert!(
        seen.iter()
            .any(|r| r["action"] == "stop_queue" && r["queue"] == "Night")
    );
    assert!(seen.iter().all(|r| r["token"] == "secret"));
}

#[test]
fn says_when_the_api_is_off_or_the_app_is_not_running() {
    let data = tempfile::tempdir().unwrap();
    fake_app(data.path(), false);
    let mut mcp = Mcp::start(data.path());
    let (text, err) = mcp.call("list_downloads", json!({}));
    assert!(err && text.contains("api on"), "{text}");

    let empty = tempfile::tempdir().unwrap();
    let mut mcp = Mcp::start(empty.path());
    let (text, err) = mcp.call("pause_downloads", json!({ "ids": [1] }));
    assert!(err && text.contains("isn't running"), "{text}");
}
