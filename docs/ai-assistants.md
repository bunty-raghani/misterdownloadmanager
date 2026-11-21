# AI assistants (MCP)

[← Back to docs index](README.md)

MDM comes with an [MCP](https://modelcontextprotocol.io) server, `misterdownloadmanager-mcp`, so an AI assistant that supports MCP (Claude Desktop, Claude Code, Cursor, VS Code and others) can work with the downloads in your running MDM. You ask in your own words:

- "What is MDM downloading right now, and how long is left?"
- "Download https://example.com/big.iso to D:\ISOs tonight, don't start it yet."
- "Pause everything that's downloading."
- "Remove the finished downloads from the list but keep the files."

The assistant starts the server itself; you only tell the assistant where it is, once. The server passes each request to the app through its [automation API](command-line.md#the-automation-api), on this computer only.

## What the assistant can do

| Tool | What it does |
|---|---|
| `list_downloads` | Lists your downloads, newest first: file, address, folder, status, size, how much is done, speed. It can list only one id, only one status (Queued, Downloading, Stopped, Complete, Error), only names and addresses that contain some text, or only one category or queue; reports available disk space too. |
| `add_download` | Adds an http, https, ftp or ftps address and starts it, or only queues it (into the main queue by default, or a designated queue). It can say which folder, file name and category to use, the checksum the finished file must have, in the same form as the command line's `--checksum` (hex MD5, SHA-1, SHA-256, SHA-512 or CRC32, optionally named, e.g. `sha256:…`), and a custom Referer, User-Agent, or Cookie header. A magnet link opens **Add torrent** in MDM, for you to choose the files. |
| `pause_downloads` | Pauses running downloads. |
| `resume_downloads` | Starts or resumes stopped, queued or failed downloads. |
| `remove_downloads` | Removes downloads from the list and keeps their files, or deletes the files too when asked. A running download must be paused first. |
| `start_queue` | Starts a download queue (the main queue by default, or by name). |
| `stop_queue` | Stops a download queue (the main queue by default, or by name). |

The assistant can't change MDM's options, see your browser, or turn the automation API on by itself.

## Set it up

### 1. Turn on the automation API

Listing and acting on downloads needs MDM's automation API, which is off until you turn it on. With MDM running, run this once (it stays on):

```
misterdownloadmanager-cli api on
```

`misterdownloadmanager-cli api off` turns it off again. Without the API, the assistant can still add downloads, the same as MDM's [command line](command-line.md#the-apps-command-line).

### 2. Find the server

`misterdownloadmanager-mcp` is next to the app:

| You installed | The server |
|---|---|
| Windows, for you only (the installer's default) | `%LOCALAPPDATA%\Programs\Mister Download Manager\misterdownloadmanager-mcp.exe` |
| Windows, for everyone | `C:\Program Files\Mister Download Manager\misterdownloadmanager-mcp.exe` |
| Windows, a zip or [portable copy](portable.md) | `misterdownloadmanager-mcp.exe` in its folder |
| macOS | `/Applications/Mister Download Manager.app/Contents/MacOS/misterdownloadmanager-mcp` |
| Linux, the .deb or .rpm | `/usr/bin/misterdownloadmanager-mcp` |
| Linux, the .tar.gz | `misterdownloadmanager-mcp` in its folder (the AppImage doesn't expose it: use the .tar.gz) |

A portable copy's server finds the portable copy's MDM by itself, from its own folder.

### 3. Tell your assistant

**Claude Desktop**: **Settings → Developer → Edit Config** opens `claude_desktop_config.json`. Add the server, then restart Claude Desktop:

```json
{
  "mcpServers": {
    "misterdownloadmanager": {
      "command": "C:\\Users\\you\\AppData\\Local\\Programs\\Mister Download Manager\\misterdownloadmanager-mcp.exe"
    }
  }
}
```

**Claude Code**:

```
claude mcp add misterdownloadmanager -- "C:\Users\you\AppData\Local\Programs\Mister Download Manager\misterdownloadmanager-mcp.exe"
```

**Cursor**: the same `mcpServers` entry as Claude Desktop's, in `~/.cursor/mcp.json` (or add it in Cursor's MCP settings).

**VS Code**: in `.vscode/mcp.json`, or **MCP: Add Server** in the command palette:

```json
{
  "servers": {
    "misterdownloadmanager": {
      "type": "stdio",
      "command": "C:\\Users\\you\\AppData\\Local\\Programs\\Mister Download Manager\\misterdownloadmanager-mcp.exe"
    }
  }
}
```

Use the path from step 2 (in JSON, every `\` is written `\\`). Other assistants take the same thing: a command to start, no arguments.

## Privacy

MDM itself still sends nothing anywhere: the server talks only to the MDM on this computer, over the token-protected local channel. But what the assistant reads, it passes to its own AI service: when it lists your downloads, their file names, addresses and folders go to that service, under that service's privacy terms. Turn the automation API off, or remove the server from the assistant, when you don't want that.

## When it doesn't work

| The assistant says | What to do |
|---|---|
| Mister Download Manager isn't running | Start MDM. The server can't start it for you. |
| The automation API is off | Run `misterdownloadmanager-cli api on` with MDM running. |
| MDM didn't answer in time | A window in MDM may be waiting for you (a question, **Add torrent**); answer it and ask again. |
| MDM refused the request | MDM's channel file is from an earlier start; restart MDM. |
| The tools don't show up at all | Check the path in the assistant's settings, and restart the assistant. Started by hand, `misterdownloadmanager-mcp --help` says what it is. |
