//! System helpers (#452): discovery, precedence and updates, with a PATH private to a child.

#[cfg(windows)]
fn main() {
    println!("system_helpers: Unix-only system discovery");
}

#[cfg(unix)]
fn main() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::Path;
    use std::process::Command;

    const ROOT: &str = "MDM_TEST_SYSTEM_HELPERS";
    if let Some(root) = std::env::var_os(ROOT) {
        check(Path::new(&root));
        println!("system_helpers: ok");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for name in ["broken", "not-executable", "bin", "package"] {
        std::fs::create_dir(root.join(name)).unwrap();
    }
    for (name, answer) in [("yt-dlp", "2026.10.26"), ("ffmpeg", "ffmpeg version 8.0")] {
        let program = root.join("package").join(name);
        std::fs::write(&program, format!("#!/bin/sh\nprintf '%s\\n' '{answer}'\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&program, root.join("bin").join(name)).unwrap();
        symlink(root.join("missing"), root.join("broken").join(name)).unwrap();
        std::fs::write(root.join("not-executable").join(name), b"not a program").unwrap();
    }
    let path =
        std::env::join_paths(["broken", "not-executable", "bin"].map(|n| root.join(n))).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .env(ROOT, root)
        .env("PATH", path)
        .status()
        .unwrap();
    assert!(status.success(), "system-helper checks failed");
}

#[cfg(unix)]
fn check(root: &std::path::Path) {
    use mdm_tools::{Installed, chosen::Helper};

    let dir = root.join("data");
    let system = Installed {
        version: "system".into(),
        exe: root.join("bin/yt-dlp"),
    };
    assert_eq!(
        mdm_tools::installed(&dir),
        Some(system.clone()),
        "find the executable, including a package-manager symlink"
    );
    assert_eq!(
        mdm_tools::ffmpeg::installed(&dir).unwrap().exe,
        root.join("bin/ffmpeg")
    );

    // The background refresh leaves the system's copy alone, and asks nothing to say so. Only an
    // explicit install goes to the network: a down adapter refuses network setup, so a regression
    // cannot contact a real release server.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let net = mdm_engine::Options {
        route: mdm_engine::Route::Down {
            label: "Offline test".into(),
        },
        ..Default::default()
    };
    assert!(!mdm_tools::refreshable(&dir), "system copy not refreshed");
    assert!(!dir.join("yt-dlp").exists(), "system helper left unmanaged");
    assert!(
        matches!(
            rt.block_on(mdm_tools::install_latest(&dir, &net, None)),
            Err(mdm_tools::Error::Adapter(mdm_engine::NetError::Down { .. }))
        ),
        "the explicit way to MDM's own copy goes to the network"
    );
    assert!(!dir.join("yt-dlp").exists(), "nothing installed on the way");

    // A managed copy still wins, and the explicit choice wins over both.
    let own = Installed {
        version: "2026.01.01".into(),
        exe: root.join("managed-yt-dlp"),
    };
    std::fs::write(&own.exe, b"managed").unwrap();
    #[cfg(target_os = "macos")]
    std::fs::create_dir(root.join("_internal")).unwrap();
    std::fs::create_dir_all(dir.join("yt-dlp")).unwrap();
    std::fs::write(
        dir.join("yt-dlp/current.json"),
        serde_json::to_vec(&own).unwrap(),
    )
    .unwrap();
    assert_eq!(mdm_tools::installed(&dir), Some(own.clone()));
    assert!(mdm_tools::refreshable(&dir), "a managed copy is refreshed");
    assert!(
        matches!(
            rt.block_on(mdm_tools::install_latest(&dir, &net, None)),
            Err(mdm_tools::Error::Adapter(mdm_engine::NetError::Down { .. }))
        ),
        "a managed copy still takes its ordinary update path"
    );

    let picked =
        mdm_tools::chosen::choose(&dir, Helper::YtDlp, &root.join("package/yt-dlp")).unwrap();
    assert_eq!(mdm_tools::installed(&dir), Some(picked.clone()));
    assert_eq!(
        rt.block_on(mdm_tools::install_latest(&dir, &net, None))
            .unwrap(),
        picked
    );
    mdm_tools::chosen::forget(&dir, Helper::YtDlp).unwrap();
    assert_eq!(mdm_tools::installed(&dir), Some(own.clone()));
    std::fs::remove_file(&own.exe).unwrap();
    assert_eq!(mdm_tools::installed(&dir), Some(system));
}
