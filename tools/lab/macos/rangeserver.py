"""A folder served over HTTP with byte ranges, for checking a built MDM where no lab server was built
(dmg-check.sh tests the release's .dmg on a runner that builds nothing). Python's standard library
only; 127.0.0.1 only.

    python3 rangeserver.py <folder> <port>
"""

import http.server
import os
import re
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    def send_head(self):
        path = self.translate_path(self.path)
        if not os.path.isfile(path):
            return super().send_head()
        size = os.path.getsize(path)
        start, end = 0, size - 1
        m = re.fullmatch(r"bytes=(\d*)-(\d*)", self.headers.get("Range", ""))
        if m and (m.group(1) or m.group(2)):
            if m.group(1):
                start = int(m.group(1))
                end = min(int(m.group(2)), size - 1) if m.group(2) else size - 1
            else:
                start = max(0, size - int(m.group(2)))
            if start > end:
                self.send_error(416)
                return None
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        else:
            self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("Last-Modified", self.date_time_string(int(os.path.getmtime(path))))
        self.end_headers()
        f = open(path, "rb")
        f.seek(start)
        self.left = end - start + 1
        return f

    def copyfile(self, source, outputfile):
        left = getattr(self, "left", None)
        while left is None or left > 0:
            chunk = source.read(65536 if left is None else min(65536, left))
            if not chunk:
                break
            outputfile.write(chunk)
            if left is not None:
                left -= len(chunk)


if __name__ == "__main__":
    folder, port = sys.argv[1], int(sys.argv[2])
    os.chdir(folder)
    http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
