//! Mister Download Manager updating itself: how this copy was installed, the release file that
//! replaces it, and that file fetched and checked against the release's SHA256SUMS.txt, whose
//! signature by MDM's release key ([`crate::signed_by_mdm`]) is checked first. Putting the file
//! in place is the app's (`crates/app/src/update.rs`): it has to close itself to do it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Error, signed_by_mdm, sum_for};

/// How this copy of MDM was installed: what an update replaces, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Install {
    /// Windows, the setup program.
    Installer,
    /// Windows, the portable zip (a `data` folder next to the program).
    PortableZip,
    /// Windows, the plain zip.
    PlainZip,
    /// macOS, the app from the .dmg.
    Dmg,
    AppImage,
    Deb,
    Rpm,
    /// Linux, the plain programs.
    TarGz,
    /// Linux, a package of another package manager (pacman, …): that package manager updates
    /// it, never MDM.
    System,
    /// Built here: never updated from a release.
    Source,
}

impl Install {
    /// This copy's, from where its program is (found once: on Linux it asks the package
    /// managers).
    pub fn detect() -> Install {
        static FOUND: std::sync::OnceLock<Install> = std::sync::OnceLock::new();
        *FOUND.get_or_init(Install::find)
    }

    fn find() -> Install {
        if cfg!(debug_assertions) {
            return Install::Source;
        }
        if mdm_ipc::portable_dir().is_some() {
            return Install::PortableZip;
        }
        let Some(exe) = std::env::current_exe().ok() else {
            return Install::Source;
        };
        let dir = exe.parent().unwrap_or(Path::new("."));
        if cfg!(windows) {
            // The installer leaves its uninstaller next to the program; the zips don't.
            return if dir.join("unins000.exe").is_file() {
                Install::Installer
            } else {
                Install::PlainZip
            };
        }
        if cfg!(target_os = "macos") {
            return if app_bundle(&exe).is_some() {
                Install::Dmg
            } else {
                Install::Source
            };
        }
        if std::env::var_os("APPIMAGE").is_some() {
            return Install::AppImage;
        }
        linux_package(&exe, &|tool, args| {
            std::process::Command::new(tool)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
    }

    /// As the bug report form words it (.github/ISSUE_TEMPLATE/bug.yml, "Installed with").
    pub fn label(self) -> &'static str {
        match self {
            Install::Installer => "The installer (setup.exe)",
            Install::PortableZip => "The portable zip",
            Install::PlainZip => "The plain zip",
            Install::Dmg => "The .dmg",
            Install::AppImage => "The AppImage",
            Install::Deb => "The .deb",
            Install::Rpm => "The .rpm",
            Install::TarGz => "The .tar.gz",
            Install::System => "Another package manager",
            Install::Source => "Built from source",
        }
    }

    pub const ALL: [Install; 10] = [
        Install::Installer,
        Install::PortableZip,
        Install::PlainZip,
        Install::Dmg,
        Install::AppImage,
        Install::Deb,
        Install::Rpm,
        Install::TarGz,
        Install::System,
        Install::Source,
    ];

    /// The release files of `version` that replace this kind of copy on this build's processor
    /// ([`Arch::THIS`]), the one to take first first: see [`Install::assets_on`].
    pub fn assets(self, version: &str) -> Vec<String> {
        self.assets_on(Arch::THIS, version)
    }

    /// The release files of `version` that replace this kind of copy built for `arch` (the names
    /// tools/installer/build.ps1, build-linux.sh and build-macos.sh give them), the one to take
    /// first first. The x64 Windows files keep the names they had before there were others
    /// (`setup.exe`, `portable.zip`, `x64.zip`), so every copy installed before finds its update;
    /// the others name their processor (`setup-arm64.exe`, `portable-x86.zip`, `arm64.zip`). The
    /// Linux files are named without "-experimental" from the release after 0.2.4
    /// (build-linux.sh), and the macOS ones may lose it too: the plain name is looked for first,
    /// then the `…-experimental` one of the releases before. None for a build from source, or for
    /// a processor MDM publishes nothing for (32-bit Linux or Mac; `None`, see [`Arch::THIS`]).
    pub fn assets_on(self, arch: Option<Arch>, version: &str) -> Vec<String> {
        let Some(arch) = arch else {
            return Vec::new();
        };
        let windows = |kind: &str| match arch {
            Arch::X64 => kind.to_owned(),
            other => format!("{kind}-{}", other.name()),
        };
        let linux = matches!(
            self,
            Install::AppImage | Install::Deb | Install::Rpm | Install::TarGz
        );
        let (tail, ext) = match (self, arch) {
            (Install::System | Install::Source, _) => return Vec::new(),
            (Install::Installer, _) => (windows("setup"), ".exe"),
            (Install::PortableZip, _) => (windows("portable"), ".zip"),
            (Install::PlainZip, _) => (arch.name().to_owned(), ".zip"),
            // One app per processor (tools/installer/build-macos.sh, #25). 0.2.4 and older look
            // for the universal one, find it missing and offer the release's page instead.
            (Install::Dmg, Arch::Arm64) => ("macos-arm64".to_owned(), ".dmg"),
            (Install::Dmg, Arch::X64) => ("macos-x86_64".to_owned(), ".dmg"),
            (_, Arch::X64) if linux => ("linux-x86_64".to_owned(), self.linux_ext()),
            (_, Arch::Arm64) if linux => ("linux-arm64".to_owned(), self.linux_ext()),
            _ => return Vec::new(),
        };
        let name = |tail: &str| format!("MisterDownloadManager-{version}-{tail}{ext}");
        match (self, arch) {
            (Install::Installer | Install::PortableZip | Install::PlainZip, _) => vec![name(&tail)],
            // Linux on ARM came after "-experimental" went: it has no older name.
            (_, Arch::Arm64) if linux => vec![name(&tail)],
            _ => vec![name(&tail), name(&format!("{tail}-experimental"))],
        }
    }

    /// A Linux file's extension.
    fn linux_ext(self) -> &'static str {
        match self {
            Install::AppImage => ".AppImage",
            Install::Deb => ".deb",
            Install::Rpm => ".rpm",
            _ => ".tar.gz",
        }
    }
}

/// The processor a release file is built for, as the Windows files' names say it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    /// 64-bit Intel and AMD (x86-64).
    X64,
    /// 64-bit ARM: Windows on ARM, Linux on ARM, Apple Silicon.
    Arm64,
    /// 32-bit Intel and AMD (Windows).
    X86,
}

impl Arch {
    /// This build's processor. A copy built for x64 and run emulated on an ARM PC says x64: it
    /// updates to the x64 files until the ARM build is installed. None for a processor MDM
    /// publishes nothing for (a build from source on RISC-V, ...): it never updates itself.
    pub const THIS: Option<Arch> = if cfg!(target_arch = "x86_64") {
        Some(Arch::X64)
    } else if cfg!(target_arch = "aarch64") {
        Some(Arch::Arm64)
    } else if cfg!(target_arch = "x86") {
        Some(Arch::X86)
    } else {
        None
    };

    /// `x64`, `arm64`, `x86`.
    pub fn name(self) -> &'static str {
        match self {
            Arch::X64 => "x64",
            Arch::Arm64 => "arm64",
            Arch::X86 => "x86",
        }
    }
}

/// Linux: which package manager's package `exe` is, asking each whether it owns the file
/// (`owns(program, arguments)`: it ran and said yes). The .deb's (dpkg), the .rpm's (rpm), or
/// another's (pacman; a program in /usr/bin none of them owns): those MDM leaves to the system's
/// updates. Else the plain programs (.tar.gz), which MDM replaces itself. A path that isn't
/// UTF-8 is never one of the packages'.
fn linux_package(exe: &Path, owns: &dyn Fn(&str, &[&str]) -> bool) -> Install {
    let Some(path) = exe.to_str() else {
        return Install::TarGz;
    };
    if owns("dpkg-query", &["--search", path]) {
        Install::Deb
    } else if owns("rpm", &["--query", "--file", path]) {
        Install::Rpm
    } else if owns("pacman", &["--query", "--owns", path])
        || exe.parent() == Some(Path::new("/usr/bin"))
    {
        Install::System
    } else {
        Install::TarGz
    }
}

/// The `.app` bundle a macOS program runs from (`X.app/Contents/MacOS/program`).
pub fn app_bundle(exe: &Path) -> Option<PathBuf> {
    let bundle = exe.parent()?.parent()?.parent()?;
    (bundle.extension().is_some_and(|e| e == "app")
        && exe.parent()?.file_name().is_some_and(|n| n == "MacOS"))
    .then(|| bundle.to_owned())
}

/// How far a download is: bytes so far, and of how many (0 while unknown).
#[derive(Default, Debug)]
pub struct Progress {
    pub got: AtomicU64,
    pub total: AtomicU64,
}

impl Progress {
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| self.got.load(Ordering::Relaxed) as f32 / total as f32)
    }
}

/// Fetch release file `name` from `assets` (name, download address) into `dir` with MDM's own
/// engine (`net`), after checking the release's SHA256SUMS.txt is signed by MDM's key, and check
/// the file against it. Returns the file.
pub async fn fetch(
    assets: &[(String, String)],
    name: &str,
    dir: &Path,
    net: &mdm_engine::Options,
    progress: &Progress,
) -> Result<PathBuf, Error> {
    let url = |file: &str| {
        assets
            .iter()
            .find(|(n, _)| n == file)
            .map(|(_, u)| u.clone())
            .ok_or_else(|| Error::Missing(file.to_owned()))
    };
    let client = crate::client(net, &url("SHA256SUMS.txt")?)?;
    let sums = crate::signed_list(
        &client,
        &url("SHA256SUMS.txt")?,
        &url("SHA256SUMS.txt.sig")?,
        signed_by_mdm,
    )
    .await?;
    let expected =
        sum_for(&sums, name).ok_or_else(|| Error::Missing(format!("checksum for {name}")))?;
    std::fs::create_dir_all(dir)?;
    let file = dir.join(name);
    // Downloaded under a name of its own, renamed once it checks out.
    let part = dir.join(format!("{name}.download"));
    crate::fetch_verified(&url(name)?, &part, expected, net, Some(progress)).await?;
    std::fs::rename(&part, &file)?;
    crate::mark_downloaded(&file, &url(name)?);
    Ok(file)
}

/// Unpack every file of `zip` into `dest`, keeping its folders (nothing can climb out of it).
pub fn unzip(zip: &Path, dest: &Path) -> Result<(), Error> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip)?)
        .map_err(|e| Error::BadRelease(e.to_string()))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| Error::BadRelease(e.to_string()))?;
        let Some(path) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(path);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::io::copy(&mut entry, &mut std::fs::File::create(&out)?)?;
    }
    Ok(())
}

/// The one folder an unpacked release holds (its files are inside it), or `dir` itself.
pub fn single_folder(dir: &Path) -> PathBuf {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    match entries.as_slice() {
        [only] if only.is_dir() => only.clone(),
        _ => dir.to_owned(),
    }
}

/// Put every file of folder `new` into `target` (the program's folder), keeping `keep` (a
/// portable copy's `data`). A file in use (the running programs) is renamed to `<name>.old`
/// first, which Windows allows, and removed on the next start ([`remove_old`]).
pub fn replace_files(new: &Path, target: &Path, keep: &[&str]) -> Result<(), Error> {
    for entry in std::fs::read_dir(new)?.flatten() {
        let name = entry.file_name();
        if keep.iter().any(|k| name == *k) {
            continue;
        }
        let (from, to) = (entry.path(), target.join(&name));
        if from.is_dir() {
            std::fs::create_dir_all(&to)?;
            replace_files(&from, &to, &[])?;
            continue;
        }
        if to.exists() {
            let old = target.join(format!("{}.old", name.to_string_lossy()));
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&to, &old)?;
        }
        if std::fs::rename(&from, &to).is_err() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Remove what [`replace_files`] set aside in `dir` (and its folders) once nothing runs it.
pub fn remove_old(dir: &Path) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.extension().is_some_and(|e| e == "old") {
                let _ = std::fs::remove_dir_all(&path);
            } else if path.file_name().is_some_and(|n| n != "data") {
                remove_old(&path);
            }
        } else if path.extension().is_some_and(|e| e == "old") {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server for `files` (published name → fixture file), one answer per connection.
    fn serve(files: Vec<(&'static str, &'static str)>) -> String {
        use std::io::{BufRead, BufReader, Write};
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut line = String::new();
                BufReader::new(&conn).read_line(&mut line).unwrap();
                let path = line
                    .split(' ')
                    .nth(1)
                    .unwrap_or("/")
                    .trim_start_matches('/');
                let body = files
                    .iter()
                    .find(|(name, _)| *name == path)
                    .and_then(|(_, f)| std::fs::read(dir.join(f)).ok());
                let mut out = &conn;
                match body {
                    Some(b) => {
                        let head = format!(
                            "HTTP/1.1 200 OK
Content-Length: {}
Connection: close

",
                            b.len()
                        );
                        let _ = out.write_all(head.as_bytes());
                        let _ = out.write_all(&b);
                    }
                    None => {
                        let _ = out.write_all(
                            b"HTTP/1.1 404 Not Found
Content-Length: 0
Connection: close

",
                        );
                    }
                }
            }
        });
        base
    }

    #[tokio::test]
    async fn a_release_file_is_taken_only_from_a_list_signed_by_mdm() {
        let zip = "MisterDownloadManager-9.9.9-x64.zip";
        let setup = "MisterDownloadManager-9.9.9-setup.exe";
        let assets = |base: &str| -> Vec<(String, String)> {
            ["SHA256SUMS.txt", "SHA256SUMS.txt.sig", zip, setup]
                .iter()
                .map(|n| (n.to_string(), format!("{base}/{n}")))
                .collect()
        };
        let base = serve(vec![
            ("SHA256SUMS.txt", "mdm-SHA256SUMS.txt"),
            ("SHA256SUMS.txt.sig", "mdm-SHA256SUMS.txt.sig"),
            (zip, zip),
            // Not what the signed list says it is.
            (setup, "mdm-SHA256SUMS.txt"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        // Through MDM's own engine, as the app fetches it.
        let net = crate::net(mdm_engine::Proxy::None);
        let progress = Progress::default();
        let file = fetch(&assets(&base), zip, dir.path(), &net, &progress)
            .await
            .unwrap();
        let len = std::fs::metadata(&file).unwrap().len();
        assert_eq!(progress.got.load(Ordering::Relaxed), len);
        assert_eq!(progress.fraction(), Some(1.0));
        let out = dir.path().join("out");
        unzip(&file, &out).unwrap();
        assert_eq!(
            std::fs::read(single_folder(&out).join("misterdownloadmanager.exe")).unwrap(),
            b"new program"
        );
        // A file that isn't the one the signed list names is refused, and not kept.
        let err = fetch(
            &assets(&base),
            setup,
            dir.path(),
            &net,
            &Progress::default(),
        )
        .await;
        assert!(matches!(err, Err(Error::BadChecksum)), "{err:?}");
        assert!(!dir.path().join(setup).exists());
        // Nothing of it is left behind (its part file, its resume state).
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(setup))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        // A list signed by another key (yt-dlp's real one) is refused.
        let base = serve(vec![
            ("SHA256SUMS.txt", "SHA2-256SUMS"),
            ("SHA256SUMS.txt.sig", "SHA2-256SUMS.sig"),
            (zip, zip),
        ]);
        let err = fetch(&assets(&base), zip, dir.path(), &net, &Progress::default()).await;
        assert!(matches!(err, Err(Error::BadSignature)), "{err:?}");
    }

    #[test]
    fn every_kind_has_its_release_file() {
        let x64 = Some(Arch::X64);
        assert_eq!(
            Install::Installer.assets_on(x64, "0.2.2"),
            ["MisterDownloadManager-0.2.2-setup.exe"]
        );
        assert_eq!(
            Install::PortableZip.assets_on(x64, "0.2.2"),
            ["MisterDownloadManager-0.2.2-portable.zip"]
        );
        assert_eq!(
            Install::PlainZip.assets_on(x64, "0.2.2"),
            ["MisterDownloadManager-0.2.2-x64.zip"]
        );
        // Linux and macOS: the plain name first, then the one up to 0.2.x.
        for (install, ext) in [
            (Install::TarGz, ".tar.gz"),
            (Install::AppImage, ".AppImage"),
            (Install::Deb, ".deb"),
            (Install::Rpm, ".rpm"),
        ] {
            assert_eq!(
                install.assets_on(x64, "1.0.0"),
                [
                    format!("MisterDownloadManager-1.0.0-linux-x86_64{ext}"),
                    format!("MisterDownloadManager-1.0.0-linux-x86_64-experimental{ext}"),
                ]
            );
        }
        // The .dmg for the Mac's own processor.
        for (arch, name) in [(Arch::Arm64, "arm64"), (Arch::X64, "x86_64")] {
            assert_eq!(
                Install::Dmg.assets_on(Some(arch), "1.0.0"),
                [
                    format!("MisterDownloadManager-1.0.0-macos-{name}.dmg"),
                    format!("MisterDownloadManager-1.0.0-macos-{name}-experimental.dmg"),
                ]
            );
        }
        assert!(Install::Source.assets_on(x64, "1.0.0").is_empty());
        // This build asks for its own processor's.
        assert_eq!(
            Install::Installer.assets("1.0.0"),
            Install::Installer.assets_on(Arch::THIS, "1.0.0")
        );
        let labels: std::collections::HashSet<_> = Install::ALL.iter().map(|i| i.label()).collect();
        assert_eq!(labels.len(), Install::ALL.len());
    }

    /// Windows on ARM and 32-bit Windows (and Linux on ARM) update to their own processor's
    /// files, never to x64's; what isn't published (32-bit Linux or Mac) is none.
    #[test]
    fn each_processor_updates_to_its_own_files() {
        let (arm, x86) = (Some(Arch::Arm64), Some(Arch::X86));
        assert_eq!(
            Install::Installer.assets_on(arm, "1.0.0"),
            ["MisterDownloadManager-1.0.0-setup-arm64.exe"]
        );
        assert_eq!(
            Install::PortableZip.assets_on(arm, "1.0.0"),
            ["MisterDownloadManager-1.0.0-portable-arm64.zip"]
        );
        assert_eq!(
            Install::PlainZip.assets_on(arm, "1.0.0"),
            ["MisterDownloadManager-1.0.0-arm64.zip"]
        );
        assert_eq!(
            Install::Installer.assets_on(x86, "1.0.0"),
            ["MisterDownloadManager-1.0.0-setup-x86.exe"]
        );
        assert_eq!(
            Install::PortableZip.assets_on(x86, "1.0.0"),
            ["MisterDownloadManager-1.0.0-portable-x86.zip"]
        );
        assert_eq!(
            Install::PlainZip.assets_on(x86, "1.0.0"),
            ["MisterDownloadManager-1.0.0-x86.zip"]
        );
        for (install, ext) in [
            (Install::TarGz, ".tar.gz"),
            (Install::AppImage, ".AppImage"),
            (Install::Deb, ".deb"),
            (Install::Rpm, ".rpm"),
        ] {
            // Linux on ARM came after "-experimental" went.
            assert_eq!(
                install.assets_on(arm, "1.0.0"),
                [format!("MisterDownloadManager-1.0.0-linux-arm64{ext}")]
            );
            assert!(install.assets_on(x86, "1.0.0").is_empty());
        }
        assert!(Install::Dmg.assets_on(x86, "1.0.0").is_empty());
        // A processor MDM publishes nothing for never updates itself.
        for install in Install::ALL {
            assert!(install.assets_on(None, "1.0.0").is_empty());
        }
        // Each kind and processor has a name of its own: no copy takes another's file.
        let mut seen = std::collections::HashMap::new();
        for arch in [Arch::X64, Arch::Arm64, Arch::X86] {
            for install in Install::ALL {
                for name in install.assets_on(Some(arch), "1.0.0") {
                    if let Some(other) = seen.insert(name.clone(), (install, arch)) {
                        panic!("{name}: {other:?} and {:?}", (install, arch));
                    }
                }
            }
        }
    }

    /// The Windows names are build.ps1's and mdm.iss's: x64's as before, the others with their
    /// processor.
    #[test]
    fn the_windows_names_are_build_ps1_s() {
        let script = include_str!("../../../tools/installer/build.ps1").replace("\r\n", "\n");
        assert!(script.contains(
            "\nNew-Zip \"MisterDownloadManager-$version-portable$suffix\" $true\nNew-Zip \"MisterDownloadManager-$version-$Arch\" $false\n"
        ));
        assert!(script.contains("$suffix = if ($Arch -eq \"x64\") { \"\" } else { \"-$Arch\" }"));
        assert!(script.contains("[ValidateSet(\"x64\", \"arm64\", \"x86\")]"));
        let iss = include_str!("../../../tools/installer/mdm.iss").replace("\r\n", "\n");
        assert!(iss.contains(
            "#if Arch == \"x64\"\nOutputBaseFilename=MisterDownloadManager-{#AppVersion}-setup\n#else\nOutputBaseFilename=MisterDownloadManager-{#AppVersion}-setup-{#Arch}\n#endif"
        ));
        for arch in [Arch::X64, Arch::Arm64, Arch::X86] {
            let (a, suffix) = match arch {
                Arch::X64 => ("x64", String::new()),
                other => (other.name(), format!("-{}", other.name())),
            };
            assert_eq!(
                Install::Installer.assets_on(Some(arch), "1.0.0"),
                [format!("MisterDownloadManager-1.0.0-setup{suffix}.exe")]
            );
            assert_eq!(
                Install::PortableZip.assets_on(Some(arch), "1.0.0"),
                [format!("MisterDownloadManager-1.0.0-portable{suffix}.zip")]
            );
            assert_eq!(
                Install::PlainZip.assets_on(Some(arch), "1.0.0"),
                [format!("MisterDownloadManager-1.0.0-{a}.zip")]
            );
        }
    }

    /// The Linux names looked for first are the ones tools/installer/build-linux.sh gives its
    /// files ("$name" plus the extension), so a release and Update now agree.
    #[test]
    fn the_linux_names_are_build_linux_sh_s() {
        let script = include_str!("../../../tools/installer/build-linux.sh").replace("\r\n", "\n");
        assert!(
            script.contains("\nname=\"MisterDownloadManager-$version-linux-$arch\"\n"),
            "build-linux.sh names its files differently"
        );
        // Its processors, as the names say them.
        for (machine, arch) in [("x86_64", "x86_64"), ("aarch64", "arm64")] {
            assert!(
                script.contains(&format!("\n  {machine}) arch={arch} ")),
                "build-linux.sh builds no {machine}"
            );
        }
        for (install, ext) in [
            (Install::TarGz, ".tar.gz"),
            (Install::AppImage, ".AppImage"),
            (Install::Deb, ".deb"),
            (Install::Rpm, ".rpm"),
        ] {
            assert!(
                script.contains(&format!("\"$dist/$name{ext}\"")),
                "build-linux.sh makes no {ext}"
            );
            assert_eq!(
                install.assets_on(Some(Arch::X64), "1.0.0")[0],
                format!("MisterDownloadManager-1.0.0-linux-x86_64{ext}")
            );
            assert_eq!(
                install.assets_on(Some(Arch::Arm64), "1.0.0")[0],
                format!("MisterDownloadManager-1.0.0-linux-arm64{ext}")
            );
        }
    }

    /// Linux: the package manager that owns the program decides; the plain programs elsewhere.
    #[test]
    fn a_linux_copy_knows_its_package_manager() {
        let owner = |who: &'static str| {
            move |tool: &str, args: &[&str]| {
                assert_eq!(args.last(), Some(&"/usr/bin/misterdownloadmanager"));
                tool == who
            }
        };
        let exe = Path::new("/usr/bin/misterdownloadmanager");
        assert_eq!(linux_package(exe, &owner("dpkg-query")), Install::Deb);
        assert_eq!(linux_package(exe, &owner("rpm")), Install::Rpm);
        // An Arch package (pacman), or in /usr/bin with no package manager MDM knows: the
        // system's.
        assert_eq!(linux_package(exe, &owner("pacman")), Install::System);
        assert_eq!(linux_package(exe, &owner("none")), Install::System);
        // On Debian with rpm's tools installed too: still the .deb's.
        let both = |tool: &str, _: &[&str]| tool == "dpkg-query" || tool == "rpm";
        assert_eq!(linux_package(exe, &both), Install::Deb);
        // The .tar.gz unpacked in the home folder, /opt or /usr/local: MDM's own to replace.
        let nobody = |_: &str, _: &[&str]| false;
        let home = Path::new("/home/me/MisterDownloadManager/misterdownloadmanager");
        assert_eq!(linux_package(home, &nobody), Install::TarGz);
        assert_eq!(
            linux_package(Path::new("/opt/mdm/misterdownloadmanager"), &nobody),
            Install::TarGz
        );
        assert_eq!(
            linux_package(Path::new("/usr/local/bin/misterdownloadmanager"), &nobody),
            Install::TarGz
        );
        // MDM updates neither another package manager's copy nor a build of its own.
        assert!(Install::System.assets("1.0.0").is_empty());
    }

    #[test]
    fn bundles_on_a_mac() {
        let exe = Path::new(
            "/Applications/Mister Download Manager.app/Contents/MacOS/misterdownloadmanager",
        );
        assert_eq!(
            app_bundle(exe).as_deref(),
            Some(Path::new("/Applications/Mister Download Manager.app"))
        );
        assert_eq!(
            app_bundle(Path::new("/usr/bin/misterdownloadmanager")),
            None
        );
    }

    #[test]
    fn files_are_replaced_and_the_old_ones_set_aside_then_removed() {
        let root = tempfile::tempdir().unwrap();
        let (new, target) = (root.path().join("new"), root.path().join("app"));
        std::fs::create_dir_all(new.join("extension")).unwrap();
        std::fs::create_dir_all(target.join("data")).unwrap();
        std::fs::write(new.join("mdm.exe"), "new").unwrap();
        std::fs::write(new.join("extension").join("manifest.json"), "new").unwrap();
        std::fs::write(new.join("data"), "must not land").unwrap();
        std::fs::write(target.join("mdm.exe"), "old").unwrap();
        std::fs::write(target.join("data").join("downloads.json"), "mine").unwrap();
        replace_files(&new, &target, &["data"]).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("mdm.exe")).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("mdm.exe.old")).unwrap(),
            "old"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("extension").join("manifest.json")).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("data").join("downloads.json")).unwrap(),
            "mine"
        );
        remove_old(&target);
        assert!(!target.join("mdm.exe.old").exists());
        assert!(target.join("mdm.exe").exists());
    }

    #[test]
    fn a_zip_unpacks_into_its_folder_and_nothing_climbs_out() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let zip_path = root.path().join("r.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("MisterDownloadManager-9.9.9-x64/mdm.exe", opts)
            .unwrap();
        w.write_all(b"exe").unwrap();
        w.start_file("../escape.txt", opts).unwrap();
        w.write_all(b"no").unwrap();
        w.finish().unwrap();
        let out = root.path().join("out");
        unzip(&zip_path, &out).unwrap();
        let folder = single_folder(&out);
        assert_eq!(std::fs::read(folder.join("mdm.exe")).unwrap(), b"exe");
        assert!(!root.path().join("escape.txt").exists());
    }
}
