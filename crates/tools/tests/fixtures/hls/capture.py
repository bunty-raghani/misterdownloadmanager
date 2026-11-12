"""Makes the HLS parity corpus again (crates/tools/src/manifest.rs, `streams_are_read_as_yt_dlp_lists_them`):
yt-dlp's own answers for streams served on this computer only. See README.md beside it.

    cargo run -p mdm-labserver -- --bind 127.0.0.1:8093
    python -I capture.py --ytdlp <yt-dlp 2026.10.26> [--out DIR]

For each case it serves the stream (the lab server's, or one of STREAMS below on 127.0.0.1:8094),
runs `yt-dlp --ignore-config --no-update --proxy "" -J --no-playlist <address>`, and writes into
`<out>/<case>/`: `master.m3u8` (the master playlist as served), `media.m3u8` (the playlist yt-dlp
asked for after it, as the servers' logs say: the one it reads the length from) and `ytdlp.json`
(its answer trimmed to what `resolve::parse` reads). Nothing is asked of any other host.
"""

import argparse
import ipaddress
import json
import os
import subprocess
import sys
import threading
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

YTDLP_VERSION = '2026.10.26'


def media(count, secs):
    """A whole (VOD) media playlist of `count` segments of `secs` seconds."""
    lines = ['#EXTM3U', '#EXT-X-VERSION:3', '#EXT-X-TARGETDURATION:10', '#EXT-X-PLAYLIST-TYPE:VOD']
    for i in range(count):
        lines += [f'#EXTINF:{secs},', f'seg{i}.ts']
    lines.append('#EXT-X-ENDLIST')
    return '\n'.join(lines) + '\n'


# The streams made for the corpus, by path on 127.0.0.1:8094 (decoded: `/my clip.m3u8`).
STREAMS = {
    '/a/de.m3u8': media(12, '9.6'),
    '/a/en.m3u8': media(12, '9.6'),
    '/aes.m3u8': (
        '#EXTM3U\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=1500000,RESOLUTION=1280x720,CODECS="avc1.4d401f,mp4a.40.2"\n'
        'v/aes.m3u8\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=600000,RESOLUTION=640x360,CODECS="avc1.42e01e,mp4a.40.2"\n'
        'v/v360.m3u8\n'
    ),
    '/group.m3u8': (
        '#EXTM3U\n'
        '#EXT-X-VERSION:4\n'
        '#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aac",NAME="English",LANGUAGE="en",DEFAULT=YES,AUTOSELECT=YES,URI="a/en.m3u8"\n'
        '#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aac",NAME="Deutsch",LANGUAGE="de",DEFAULT=NO,AUTOSELECT=YES,URI="a/de.m3u8"\n'
        '#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="Deutsch",LANGUAGE="de",URI="s/de.m3u8"\n'
        '#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="Francais",LANGUAGE="fr",URI="s/fr.m3u8"\n'
        '#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="Unknown",URI="s/fr.m3u8"\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=5000000,AVERAGE-BANDWIDTH=4500000,RESOLUTION=1920x1080,FRAME-RATE=60.000,CODECS="avc1.640028,mp4a.40.2",AUDIO="aac",SUBTITLES="subs"\n'
        'v/v1080.m3u8\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=2800000,RESOLUTION=1280x720,FRAME-RATE=30,CODECS="avc1.4d401f,mp4a.40.2",AUDIO="aac",SUBTITLES="subs"\n'
        'v/v720.m3u8\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,CODECS="avc1.42e01e,mp4a.40.2",AUDIO="aac",SUBTITLES="subs"\n'
        'v/v360.m3u8\n'
    ),
    '/my clip.m3u8': (
        '#EXTM3U\n'
        '#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="Main",DEFAULT=YES\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=1800000,RESOLUTION=720x1280,CODECS="avc1.4d401f,mp4a.40.2",AUDIO="aud"\n'
        'v/v720.m3u8\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=700000,RESOLUTION=360x640,CODECS="avc1.42e01e,mp4a.40.2",AUDIO="aud"\n'
        'v/v360.m3u8\n'
    ),
    '/nocodecs.m3u8': (
        '#EXTM3U\n'
        '#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="English",LANGUAGE="en",URI="a/en.m3u8"\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1280x720,AUDIO="aud"\n'
        'v/v720.m3u8\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=640x360,AUDIO="aud"\n'
        'v/v360.m3u8\n'
    ),
    '/order.m3u8': (
        '#EXTM3U\n'
        '#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="English",LANGUAGE="en",URI="a/en.m3u8"\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=480x270,CODECS="avc1.42e01e",AUDIO="aud"\n'
        'v/v480.m3u8\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=6000000,RESOLUTION=1920x1080,CODECS="avc1.640028",AUDIO="aud"\n'
        'v/v1080.m3u8\n'
    ),
    '/repeated.m3u8': (
        '#EXTM3U\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=2000000,AVERAGE-BANDWIDTH=1800000,RESOLUTION=1280x720,FRAME-RATE=30.000,CODECS="avc1.4d401f,mp4a.40.2"\n'
        'v/v720.m3u8\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=4000000,RESOLUTION=1280x720,FRAME-RATE=59.940,CODECS="avc1.4d401f,mp4a.40.2"\n'
        'v/v720b.m3u8\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=1234567,RESOLUTION=854x480,FRAME-RATE=29.97,CODECS="avc1.4d401e,mp4a.40.2"\n'
        'v/v480.m3u8\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=700000,RESOLUTION=854x480,CODECS="avc1.4d401e,mp4a.40.2"\n'
        'v/v360.m3u8\n'
    ),
    '/s/de.m3u8': media(3, '10'),
    '/s/fr.m3u8': media(3, '10'),
    '/v/aes.m3u8': (
        '#EXTM3U\n'
        '#EXT-X-TARGETDURATION:10\n'
        '#EXT-X-KEY:METHOD=AES-128,URI="key.bin",KEYFORMAT="identity"\n'
        '#EXTINF:6.006,\n'
        'seg0.ts\n'
        '#EXTINF:6.006,\n'
        'seg1.ts\n'
        '#EXTINF:6.006,\n'
        'seg2.ts\n'
        '#EXTINF:6.006,\n'
        'seg3.ts\n'
        '#EXTINF:6.006,\n'
        'seg4.ts\n'
        '#EXT-X-ENDLIST\n'
    ),
    '/v/v1080.m3u8': media(10, '9.009'),
    '/v/v360.m3u8': media(3, '10.0'),
    '/v/v480.m3u8': media(7, '9.9'),
    '/v/v720.m3u8': media(10, '9.5'),
    '/v/v720b.m3u8': media(10, '9.5'),
    '/videoonly.m3u8': (
        '#EXTM3U\n'
        '#EXT-X-INDEPENDENT-SEGMENTS\n'
        '#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a1",NAME="main",LANGUAGE="en",DEFAULT=YES,URI="a/en.m3u8"\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1280x720,CODECS="avc1.4d401f",AUDIO="a1"\n'
        'v/v720.m3u8\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=1200000,RESOLUTION=854x480,CODECS="avc1.4d401e",AUDIO="a1"\n'
        'v/v480.m3u8\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=96000,CODECS="mp4a.40.2"\n'
        'a/de.m3u8\n'
        '#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=300000,RESOLUTION=1280x720,CODECS="avc1.4d401f",URI="v/v720b.m3u8"\n'
        '#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=100000,RESOLUTION=854x480,CODECS="avc1.4d401e",URI="v/v480.m3u8"\n'
    ),
}

# Each case: its folder, where its master playlist is (`own`: STREAMS; `lab`: the lab server;
# `redirect`: the lab server's /redirect to one of STREAMS) and its path there.
CASES = [
    ('lab', 'lab', '/hls/master.m3u8'),
    ('group', 'own', '/group.m3u8'),
    ('videoonly', 'own', '/videoonly.m3u8'),
    ('repeated', 'own', '/repeated.m3u8'),
    ('nocodecs', 'own', '/nocodecs.m3u8'),
    ('aes', 'own', '/aes.m3u8'),
    ('portrait', 'own', '/my%20clip.m3u8'),
    ('order', 'own', '/order.m3u8'),
    ('redirect', 'redirect', '/repeated.m3u8'),
]

# What resolve::parse reads of yt-dlp's answer.
TOP = ['title', 'duration', 'direct', 'language']
SUBS = ['subtitles', 'automatic_captions']
FORMAT = ['format_id', 'ext', 'height', 'fps', 'vcodec', 'acodec', 'filesize', 'filesize_approx', 'tbr', 'protocol']


def trim(info):
    out = {k: info[k] for k in TOP if k in info}
    for k in SUBS:
        if info.get(k) is not None:
            out[k] = {lang: [({'name': s['name']} if 'name' in s else {}) for s in subs]
                      for lang, subs in info[k].items()}
    out['formats'] = [{k: f[k] for k in FORMAT if k in f} for f in info['formats']]
    return json.dumps(out, indent=1, ensure_ascii=False) + '\n'


class Streams(BaseHTTPRequestHandler):
    def do_GET(self):
        self.server.seen.append(urllib.parse.urlsplit(self.path).path)
        body = STREAMS.get(urllib.parse.unquote(urllib.parse.urlsplit(self.path).path))
        if body is None:
            self.send_error(404)
            return
        data = body.encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/vnd.apple.mpegurl')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


def loopback(url):
    host = urllib.parse.urlsplit(url).hostname or ''
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return host == 'localhost'


def get(url, method='GET'):
    assert loopback(url), url
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(
            urllib.request.Request(url, method=method), timeout=10) as r:
        return r.read()


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--ytdlp', required=True, help='yt-dlp ' + YTDLP_VERSION)
    ap.add_argument('--lab', default='http://127.0.0.1:8093', help='the lab server (mdm-labserver)')
    ap.add_argument('--port', type=int, default=8094, help="the corpus's own streams' port")
    ap.add_argument('--out', default=os.path.dirname(os.path.abspath(__file__)))
    a = ap.parse_args()
    if not loopback(a.lab):
        sys.exit('the lab server must be on this computer')
    version = subprocess.run([a.ytdlp, '--version'], capture_output=True, text=True).stdout.strip()
    if version != YTDLP_VERSION:
        print(f'note: yt-dlp {version}, the corpus is {YTDLP_VERSION}', file=sys.stderr)
    server = ThreadingHTTPServer(('127.0.0.1', a.port), Streams)
    server.seen = []
    threading.Thread(target=server.serve_forever, daemon=True).start()
    own = f'http://127.0.0.1:{a.port}'
    lab = a.lab.rstrip('/')
    try:
        for case, where, path in CASES:
            if where == 'lab':
                address = lab + path
            elif where == 'redirect':
                address = lab + '/redirect?to=' + urllib.parse.quote(own + path, safe='')
            else:
                address = own + path
            server.seen.clear()
            get(lab + '/_log', 'DELETE')
            cmd = [a.ytdlp, '--ignore-config', '--no-update', '--proxy', '', '-J', '--no-playlist', address]
            print(' '.join(c if c else '""' for c in cmd[1:]), file=sys.stderr)
            answer = subprocess.run(cmd, capture_output=True, timeout=120, check=True).stdout
            info = json.loads(answer)
            # The playlist yt-dlp asked for after the master: the first other one in the logs.
            if where == 'lab':
                seen = [e['path'] for e in json.loads(get(lab + '/_log')) if e['method'] == 'GET']
                base = lab
            else:
                seen, base = list(server.seen), own
            after = [p for p in seen if p != path]
            master = get(base + path)
            first = get(base + after[0])
            folder = os.path.join(a.out, case)
            os.makedirs(folder, exist_ok=True)
            for name, data in [('master.m3u8', master), ('media.m3u8', first)]:
                with open(os.path.join(folder, name), 'wb') as f:
                    f.write(data)
            with open(os.path.join(folder, 'ytdlp.json'), 'w', encoding='utf-8', newline='\n') as f:
                f.write(trim(info))
            print(f'{case}: {address} -> {base}{after[0]}', file=sys.stderr)
    finally:
        server.shutdown()


if __name__ == '__main__':
    main()
