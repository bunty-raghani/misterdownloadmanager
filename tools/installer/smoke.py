"""A release build's programs, run on the machine that built them (release.yml) before they are
packed: each is a program for the processor its files are named for, and the app downloads a file
and gets it whole.

    python tools/installer/smoke.py <folder with the programs> <x64|arm64|x86>

The app's command line (`misterdownloadmanager --cli --json get`) downloads 3 MB from a server on
this machine (byte ranges, so several connections) with a data folder and a download folder of its
own, and the file must be the one served; misterdownloadmanager-mcp --version must name the
version. Nothing is left behind.
"""
import hashlib
import http.server
import json
import os
import random
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import threading

# Each processor as Windows (PE) and Linux (ELF) programs say it.
PE = {"x64": 0x8664, "arm64": 0xAA64, "x86": 0x014C}
ELF = {"x64": 62, "arm64": 183}
PROGRAMS = ["misterdownloadmanager", "misterdownloadmanager-nmhost", "misterdownloadmanager-cli",
            "misterdownloadmanager-mcp"]
SIZE = 3 << 20


def machine(path):
    """("pe" or "elf", its machine number)."""
    with open(path, "rb") as f:
        head = f.read(4096)
    if head[:2] == b"MZ":
        pe = struct.unpack_from("<I", head, 0x3C)[0]
        if head[pe:pe + 4] != b"PE\0\0":
            sys.exit(f"{path}: not a Windows program")
        return "pe", struct.unpack_from("<H", head, pe + 4)[0]
    if head[:4] == b"\x7fELF":
        return "elf", struct.unpack_from("<H", head, 18)[0]
    sys.exit(f"{path}: neither a Windows nor a Linux program")


def serve(body):
    """A server on 127.0.0.1 for `body` at /file, with byte ranges; its address."""

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

        def do_HEAD(self):
            self.answer(head=True)

        def do_GET(self):
            self.answer(head=False)

        def answer(self, head):
            if self.path != "/file":
                self.send_response(404)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            start, end = 0, len(body) - 1
            m = re.fullmatch(r"bytes=(\d+)-(\d*)", self.headers.get("Range", ""))
            if m:
                start = int(m.group(1))
                end = min(int(m.group(2)) if m.group(2) else end, len(body) - 1)
                self.send_response(206)
                self.send_header("Content-Range", f"bytes {start}-{end}/{len(body)}")
            else:
                self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Length", str(end - start + 1))
            self.end_headers()
            if not head:
                self.wfile.write(body[start:end + 1])

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_address[1]}/file"


def main():
    if len(sys.argv) != 3 or sys.argv[2] not in PE:
        sys.exit(__doc__)
    folder, arch = sys.argv[1], sys.argv[2]
    windows = sys.platform == "win32"
    exe = ".exe" if windows else ""
    want = ("pe", PE[arch]) if windows else ("elf", ELF.get(arch))
    for p in PROGRAMS:
        got = machine(os.path.join(folder, p + exe))
        if got != want:
            sys.exit(f"{p}: built for machine {got[1]:#x}, not {arch} ({want[1]:#x})")
        print(f"{p}{exe}: {arch}")

    version = re.search(r'(?m)^version\s*=\s*"([^"]+)"',
                        open(os.path.join(os.path.dirname(__file__), "..", "..", "Cargo.toml"),
                             encoding="utf-8").read()).group(1)
    out = subprocess.run([os.path.join(folder, "misterdownloadmanager-mcp" + exe), "--version"],
                         capture_output=True, text=True, timeout=60)
    if out.returncode or version not in out.stdout:
        sys.exit(f"misterdownloadmanager-mcp --version: {out.returncode} {out.stdout!r} {out.stderr!r}")
    print(out.stdout.strip())

    body = random.Random(668).randbytes(SIZE)
    server, url = serve(body)
    work = tempfile.mkdtemp(prefix="mdm-smoke-")
    try:
        data, dest = os.path.join(work, "data"), os.path.join(work, "downloads", "smoke.bin")
        os.makedirs(data)
        os.makedirs(os.path.dirname(dest))
        env = dict(os.environ, MDM_DATA_DIR=data, MDM_DOWNLOADS=os.path.dirname(dest))
        out = subprocess.run(
            [os.path.join(folder, "misterdownloadmanager" + exe), "--cli", "--json", "get", url,
             "-o", dest, "--checksum", "sha256:" + hashlib.sha256(body).hexdigest()],
            capture_output=True, text=True, timeout=300, env=env)
        events = [json.loads(line) for line in out.stdout.splitlines() if line.strip()]
        if out.returncode or not events or events[-1].get("event") != "done":
            sys.exit(f"the download failed ({out.returncode}): {out.stdout[-2000:]} {out.stderr[-2000:]}")
        with open(dest, "rb") as f:
            if hashlib.sha256(f.read()).digest() != hashlib.sha256(body).digest():
                sys.exit("the downloaded file isn't the one served")
        print(f"downloaded {SIZE} bytes whole: {events[-1]}")
    finally:
        server.shutdown()
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
