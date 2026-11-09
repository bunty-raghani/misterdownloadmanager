"""The release's symbols for crash reports: Breakpad .sym files of the programs MDM ships, in a
symbol store (<debug file>/<id>/<name>.sym, as rust-minidump and Mozilla's symbol servers read
it), zipped as MisterDownloadManager-<version>-symbols-<platform>.zip beside the release's other
downloads. The crash report window fetches the zip for its own version to make its stacks
readable (crates/crash/src/symbols.rs); the programs themselves ship without their debug info.

    python tools/installer/symbols.py <platform> <version> <out dir> <file>[:<arch>] ...

<file> is a program built with debug info (Windows: its .pdb beside it; Linux: an ELF before
stripping; macOS: a .dSYM, with :x86_64 / :arm64 to pick one half of a universal one).
dump_syms (Mozilla, pinned below) is fetched and its checksum checked first.
"""
import hashlib
import io
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from fetch_release import fetch  # noqa: E402  (beside this file: signs in, tries again)

DUMP_SYMS = "v2.3.9"


def fetch_dump_syms(work):
    if sys.platform == "win32":
        name = "dump_syms-x86_64-pc-windows-msvc.zip"
    elif sys.platform == "darwin":
        name = ("dump_syms-aarch64-apple-darwin.tar.xz" if platform.machine() == "arm64"
                else "dump_syms-x86_64-apple-darwin.tar.xz")
    else:
        name = "dump_syms-x86_64-unknown-linux-gnu.tar.xz"
    data = fetch("mozilla/dump_syms", DUMP_SYMS, name)
    want = fetch("mozilla/dump_syms", DUMP_SYMS, name + ".sha256").decode().split()[0]
    got = hashlib.sha256(data).hexdigest()
    if got != want:
        sys.exit(f"dump_syms checksum mismatch: {got} != {want}")
    exe = "dump_syms.exe" if sys.platform == "win32" else "dump_syms"
    if name.endswith(".zip"):
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            member = next(m for m in z.namelist() if m.endswith(exe))
            out = os.path.join(work, exe)
            with open(out, "wb") as f:
                f.write(z.read(member))
    else:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:xz") as t:
            member = next(m for m in t.getmembers() if m.name.endswith("/" + exe) or m.name == exe)
            out = os.path.join(work, exe)
            with open(out, "wb") as f:
                f.write(t.extractfile(member).read())
        os.chmod(out, 0o755)
    return out


def main():
    if len(sys.argv) < 5:
        sys.exit(__doc__)
    plat, version, out_dir = sys.argv[1:4]
    inputs = sys.argv[4:]
    work = tempfile.mkdtemp()
    store = os.path.join(work, "store")
    dump_syms = fetch_dump_syms(work)
    for item in inputs:
        # Only a trailing :x86_64 / :arm64 picks an architecture (C:\… is a drive letter).
        path, arch = item, ""
        for a in ("x86_64", "arm64"):
            if item.endswith(":" + a):
                path, arch = item[: -len(a) - 1], a
        cmd = [dump_syms, path] + (["-a", arch] if arch else [])
        sym = subprocess.run(cmd, check=True, capture_output=True).stdout
        first = sym.split(b"\n", 1)[0].decode().split()
        # MODULE <os> <arch> <id> <debug file>
        if len(first) != 5 or first[0] != "MODULE":
            sys.exit(f"{path}: no MODULE line from dump_syms")
        debug_id, debug_file = first[3], first[4]
        name = debug_file[:-4] if debug_file.lower().endswith(".pdb") else debug_file
        dest = os.path.join(store, debug_file, debug_id)
        os.makedirs(dest, exist_ok=True)
        with open(os.path.join(dest, name + ".sym"), "wb") as f:
            f.write(sym)
        print(f"{debug_file} {debug_id} ({len(sym) / 1048576:.1f} MB)")
    os.makedirs(out_dir, exist_ok=True)
    zip_path = os.path.join(out_dir, f"MisterDownloadManager-{version}-symbols-{plat}.zip")
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for root, _, files in os.walk(store):
            for fn in files:
                full = os.path.join(root, fn)
                z.write(full, os.path.relpath(full, store).replace(os.sep, "/"))
    print(f"{zip_path} ({os.path.getsize(zip_path) / 1048576:.1f} MB)")
    shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
