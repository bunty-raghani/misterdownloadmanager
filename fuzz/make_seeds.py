"""Writes the fuzz targets' seed corpora (fuzz/seeds/<target>/) from the repository's own test
fixtures and a few small hand-made inputs. Run from anywhere: python fuzz/make_seeds.py

The seeds are committed; run this again after a fixture they come from changes."""

import gzip
import io
import lzma
import tarfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEEDS = ROOT / "fuzz" / "seeds"
TOOLS = ROOT / "crates" / "tools" / "tests" / "fixtures"
ENGINE = ROOT / "crates" / "engine" / "tests" / "data"
SEP = b"\n--mdm-fuzz--\n"  # fuzz_targets/signature.rs


def seed(target: str, name: str, data: bytes) -> None:
    d = SEEDS / target
    d.mkdir(parents=True, exist_ok=True)
    (d / name).write_bytes(data)


def bencode(v) -> bytes:
    if isinstance(v, int):
        return b"i%de" % v
    if isinstance(v, str):
        v = v.encode()
    if isinstance(v, bytes):
        return b"%d:%s" % (len(v), v)
    if isinstance(v, list):
        return b"l" + b"".join(bencode(x) for x in v) + b"e"
    return b"d" + b"".join(bencode(k) + bencode(v[k]) for k in sorted(v)) + b"e"


def hls() -> None:
    for case in sorted(p for p in (TOOLS / "hls").iterdir() if p.is_dir()):
        master = (case / "master.m3u8").read_bytes()
        media = (case / "media.m3u8").read_bytes()
        seed("hls", case.name, master + b"\0" + media)
    seed(
        "hls",
        "byterange-key",
        b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:7\n"
        b'#EXT-X-MAP:URI="init.mp4",BYTERANGE="720@0"\n'
        b'#EXT-X-KEY:METHOD=AES-128,URI="k.bin",IV=0x0102030405060708090a0b0c0d0e0f10\n'
        b"#EXTINF:4.0,\n#EXT-X-BYTERANGE:1000@720\nmedia.mp4\n"
        b"#EXTINF:4.0,\n#EXT-X-BYTERANGE:1000\nmedia.mp4\n"
        b"#EXT-X-KEY:METHOD=NONE\n#EXTINF:2.5,\nlast.ts?x=1\n#EXT-X-ENDLIST\n",
    )
    # Numbers at a u64's edge (a part's last byte, the pieces' numbers): read, never overflowed.
    seed(
        "hls",
        "u64-edge",
        b"#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:18446744073709551615\n"
        b"#EXT-X-BYTERANGE:18446744073709551615@18446744073709551615\n#EXTINF:4,\na.ts\n"
        b"#EXT-X-BYTERANGE:1\n#EXTINF:4,\nb.ts\n#EXT-X-ENDLIST\n",
    )


def ytdlp() -> None:
    for case in sorted(p for p in (TOOLS / "hls").iterdir() if p.is_dir()):
        seed("ytdlp_json", f"hls-{case.name}", (case / "ytdlp.json").read_bytes())
    seed("ytdlp_json", "youtube-portrait", (TOOLS / "youtube-4k60-portrait.json").read_bytes())
    seed(
        "ytdlp_json",
        "dash-script",
        b'{"title":"t","duration":12.5,"extractor_key":"Generic","formats":['
        b'{"format_id":"video/mp4","ext":"php","url":"https://a.example/stream.php?file=x.mp4",'
        b'"vcodec":"avc1","acodec":"mp4a","filesize_approx":-5,"tbr":800,"height":720},'
        b'{"format_id":"dash-1","ext":"m4a","vcodec":"none","acodec":"opus","protocol":"http_dash_segments",'
        b'"fragment_base_url":"https://cdn.example/dash/v1/","fragments":[{"path":"init.mp4"},'
        b'{"url":"https://cdn.example/x/seg1.m4s"},{"path":"s2.m4s","byte_range":{"start":10,"end":20}},'
        b'{"path":"s3.m4s","range":"20-29"}]}],'
        b'"subtitles":{"en":[{"name":"English"}]},"automatic_captions":{"de-orig":[{}]},"language":"de"}',
    )
    seed("ytdlp_json", "playlist", b'{"_type":"playlist","title":" A list ","playlist_count":3}')
    seed(
        "ytdlp_json",
        "dash-u64-edge",
        b'{"formats":[{"format_id":"d","fragment_base_url":"https://c.example/","fragments":['
        b'{"path":"a","range":"0-18446744073709551615"},'
        b'{"path":"b","byte_range":{"start":18446744073709551615,"end":18446744073709551615}}]}]}',
    )


def web() -> None:
    for name in ["declared-video.html", "drive-warning.html", "drive-quota.html", "drive-old.html"]:
        seed("web_page", name, (ENGINE / name).read_bytes())
    seed(
        "web_page",
        "links",
        b'<html><head><base href="/files/"><meta property="og:video" content="https://v.example/a.mp4">'
        b'<meta name="twitter:player" content="https://p.example/embed/1"></head><body>'
        b'<a href="a.zip">a</a><a href="ftp://ftp.example/b.iso">b</a><area href="magnet:?xt=urn:btih:00">'
        b'<video src="clip.mp4#t=1"><source src="clip.webm"></video><!-- <a href="no.zip"> -->'
        b'<script>var s = "<a href=\'no2.zip\'>";</script><a href="javascript:x()">j</a>'
        b'<a href="&#104;ttps://e.example/&amp;q">e</a></body></html>',
    )


def names() -> None:
    for i, lines in enumerate(
        [
            [b'attachment; filename="report.pdf"', b"https://a.example/dl?id=3", b"application/pdf", b"Report"],
            [b"attachment; filename*=UTF-8''na%C3%AFve%20file.txt; filename=\"naive.txt\"", b"https://a.example/x/"],
            [b"inline; filename=..\\..\\CON.txt", b"https://a.example/stream.php?file=clip.mp4", b"video/mp4"],
            [b"filename*=iso-8859-1'en'%A3%20rates.xls", b"https://a.example/%E2%80%AEfdp.exe", b"", b"\xe2\x80\xae Title: of / a page?"],
            [b"", b"https://a.example/videoplayback?expire=1", b"video/webm", b"A long, long title " * 12],
            [b"", b"ftp://a.example/dir/archive.tar.gz"],
            [b"attachment; filename=\"a\\\"b;c.zip\"; size=3", b"/relative/name.", b"text/html"],
            [b"COM1 .txt", b"dir\\sub/../.\\x:stream", b"audio/mpeg"],
        ]
    ):
        seed("file_name", f"lines-{i}", b"\n".join(lines))
    seed("file_name", "mp4-head", b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom")
    seed("file_name", "ebml-head", b"\x1aE\xdf\xa3\x9fB\x86\x81\x01webm")


def torrents() -> None:
    piece = b"\x11" * 20
    single = {
        "announce": "udp://tracker.example.org:1337/announce",
        "info": {"name": "file.bin", "length": 32768, "piece length": 16384, "pieces": piece * 2},
    }
    multi = {
        "announce": "http://127.0.0.1:8080/announce",
        "announce-list": [["http://t1.example/announce", "udp://10.0.0.1:6969"], ["https://t2.example/a"]],
        "comment": "seed",
        "info": {
            "name": "folder",
            "piece length": 16384,
            "pieces": piece,
            "files": [
                {"length": 10, "path": ["a", "b.txt"]},
                {"length": 5, "path": ["c.txt"]},
                {"length": 100, "path": [".pad", "100"], "attr": "p"},
            ],
        },
    }
    seed("torrent", "single", bencode(single))
    seed("torrent", "multi", bencode(multi))
    seed(
        "torrent",
        "magnet",
        b"magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=x"
        b"&tr=udp%3A%2F%2Ftracker.example.org%3A1337&tr=http%3A%2F%2F192.168.1.2%2Fa&tr=",
    )


def signatures() -> None:
    keys = TOOLS.parent.parent / "keys"
    mdm = (TOOLS / "mdm-SHA256SUMS.txt").read_bytes()
    seed("signature", "mdm", b"\x00" + (TOOLS / "mdm-SHA256SUMS.txt.sig").read_bytes() + SEP + mdm)
    seed(
        "signature",
        "ytdlp",
        b"\x01" + (TOOLS / "SHA2-256SUMS.sig").read_bytes() + SEP + (TOOLS / "SHA2-256SUMS").read_bytes(),
    )
    seed(
        "signature",
        "gallery-dl",
        b"\x02"
        + (TOOLS / "gallery-dl-SHA256SUMS.sig").read_bytes()
        + SEP
        + (TOOLS / "gallery-dl-SHA256SUMS").read_bytes(),
    )
    seed(
        "signature",
        "mdm-key",
        b"\x03"
        + (keys / "mdm-release.asc").read_bytes()
        + SEP
        + (TOOLS / "mdm-SHA256SUMS.txt.sig").read_bytes()
        + SEP
        + mdm,
    )


def cnl() -> None:
    jk = b"function f(){ return '31323334353637383930313233343536'; }"
    seed(
        "click_n_load",
        "links",
        jk + b"\nhttps://files.example.com/a.rar\r\nhttps://files.example.com/a.rar\n"
        b"ftp://ftp.example.org/b.zip\n\x00\x00http://192.168.0.1/x\nhttp://printer/\nmailto:x@y\n",
    )
    seed("click_n_load", "crypted", b'function f(){return "00112233445566778899aabbccddeeff"}\nq83vEjRWeJA=\n')


def archives() -> None:
    def tar(mode: str) -> bytes:
        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode=mode) as t:
            for name, data in [("dir/a.txt", b"hello\n"), ("../up.txt", b"x"), ("C:evil", b"y")]:
                info = tarfile.TarInfo(name)
                info.size = len(data)
                t.addfile(info, io.BytesIO(data))
            link = tarfile.TarInfo("link")
            link.type = tarfile.SYMTYPE
            link.linkname = "/etc"
            t.addfile(link)
        return buf.getvalue()

    zbuf = io.BytesIO()
    with zipfile.ZipFile(zbuf, "w", zipfile.ZIP_DEFLATED) as z:
        # A fixed date, so that the seed is the same each time.
        for name, data in [("dir/a.txt", b"hello\n" * 20), ("..\\up.txt", b"x"), ("CON.txt", b"y")]:
            z.writestr(zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0)), data, zipfile.ZIP_DEFLATED)
    plain = tar("w")
    seed("archive", "zip", b"\x00" + zbuf.getvalue())
    seed("archive", "tar", b"\x01" + plain)
    seed("archive", "tar-gz", b"\x02" + gzip.compress(plain, mtime=0))
    seed("archive", "tar-xz", b"\x03" + lzma.compress(plain))
    seed("archive", "gz", b"\x04" + gzip.compress(b"one file\n", mtime=0))
    seed("archive", "xz", b"\x05" + lzma.compress(b"one file\n"))


if __name__ == "__main__":
    hls()
    ytdlp()
    web()
    names()
    torrents()
    signatures()
    cnl()
    archives()
