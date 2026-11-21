"""The release's symbols for crash reports: Breakpad .sym files of the programs MDM ships, in a
symbol store (<debug file>/<id>/<name>.sym, as rust-minidump and Mozilla's symbol servers read
it), zipped as MisterDownloadManager-<version>-symbols-<platform>.zip beside the release's other
downloads. The crash report window fetches the zip for its own version to make its stacks
readable (crates/crash/src/symbols.rs); the programs themselves ship without their debug info.

    python tools/installer/symbols.py <platform> <version> <out dir> <file>[+<program>][:<arch>] ...

<file> is a program built with debug info (Windows: its .pdb beside it; Linux: an ELF before
stripping; macOS: a .dSYM, with :x86_64 / :arm64 to pick one half of a universal one).
macOS: <program>.dSYM+<program>, the dSYM with the program it was made from, before stripping.
The dSYM has the names, lines and inlined calls; the program its unwinding tables (__unwind_info,
__eh_frame), which dsymutil leaves out: without them (STACK CFI) the crash report's unwinder
guesses from frame pointers and skips frames (#1515). Both are read into one .sym, with its
inlined calls (INLINE), named after the program as a dump names it.
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
    # The build machine's: it reads every processor's programs (Windows on ARM runs the x64 one
    # emulated, as dump_syms has no ARM build for Windows).
    if sys.platform == "win32":
        name = "dump_syms-x86_64-pc-windows-msvc.zip"
    elif sys.platform == "darwin":
        name = ("dump_syms-aarch64-apple-darwin.tar.xz" if platform.machine() == "arm64"
                else "dump_syms-x86_64-apple-darwin.tar.xz")
    elif platform.machine() in ("aarch64", "arm64"):
        name = "dump_syms-aarch64-unknown-linux-gnu.tar.xz"
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
        # Only "+" right after a .dSYM names its program (other paths may hold a "+").
        program = ""
        if ".dSYM+" in path:
            path, program = path.split(".dSYM+", 1)
            path += ".dSYM"
        dsym = path.rstrip("/").endswith(".dSYM")
        cmd = [dump_syms, path] + ([program] if program else []) + (["-a", arch] if arch else [])
        if dsym:
            cmd.append("--inlines")
        if program:
            # The program's unwinding tables are why it is here: fail rather than ship without.
            cmd.append("--check-cfi")
        run = subprocess.run(cmd, capture_output=True)
        if run.returncode:
            sys.exit(f"{path}: dump_syms failed: {run.stderr.decode(errors='replace').strip()}")
        sym = run.stdout
        head, _, rest = sym.partition(b"\n")
        first = head.decode().split()
        # MODULE <os> <arch> <id> <debug file>
        if len(first) != 5 or first[0] != "MODULE":
            sys.exit(f"{path}: no MODULE line from dump_syms")
        debug_id, debug_file = first[3], first[4]
        if debug_file.endswith(".dSYM"):
            # dump_syms names a dSYM's module after the bundle; a macOS dump names it after the
            # program, the name the report window's lookup goes by (<debug file>/<id>/<name>.sym):
            # under the bundle's name its symbols were never found.
            debug_file = debug_file[: -len(".dSYM")]
            first[4] = debug_file
            sym = " ".join(first).encode() + b"\n" + rest
        name = debug_file[:-4] if debug_file.lower().endswith(".pdb") else debug_file
        dest = os.path.join(store, debug_file, debug_id)
        os.makedirs(dest, exist_ok=True)
        with open(os.path.join(dest, name + ".sym"), "wb") as f:
            f.write(sym)
        lines = sym.split(b"\n")
        cfi = sum(1 for line in lines if line.startswith(b"STACK CFI"))
        inline = sum(1 for line in lines if line.startswith(b"INLINE "))
        print(f"{debug_file} {debug_id} ({len(sym) / 1048576:.1f} MB, "
              f"{cfi} STACK CFI, {inline} INLINE)")
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
