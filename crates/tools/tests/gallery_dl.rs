//! Real gallery-dl releases against the lab server, as MDM starts them (#236: a distribution's
//! gallery-dl may be older than MDM's own copy). Each release MDM can drive is chosen, reads an
//! address (and says when it can't), and saves a picture with MDM's User-Agent; one older than
//! `gallery::OLDEST` is refused when chosen.
//!
//! Ignored: it needs `MDM_TEST_GALLERYDLS`, the programs separated as in PATH (`:` or `;`):
//!
//!   MDM_TEST_GALLERYDLS=$HOME/gdl/1.20.0/bin/gallery-dl:/usr/bin/gallery-dl \
//!     cargo test -p mdm-tools --test gallery_dl -- --ignored --nocapture

use std::time::{Duration, Instant};

use mdm_labserver::{Config, LabServer};
use mdm_tools::chosen::Helper;
use mdm_tools::gallery::{Ask, GalleryJob, USER_AGENT};
use mdm_tools::video::State;

#[test]
#[ignore]
fn every_release_mdm_can_drive_saves_a_gallery() {
    let list = std::env::var_os("MDM_TEST_GALLERYDLS").expect("set MDM_TEST_GALLERYDLS");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let lab = rt.block_on(LabServer::start(Config::default())).unwrap();
    let mut rows = Vec::new();
    for exe in std::env::split_paths(&list) {
        let asked = mdm_tools::chosen::ask(Helper::GalleryDl, &exe)
            .unwrap_or_else(|e| panic!("{}: {e}", exe.display()));
        let v = asked.version.clone();
        let data = tempfile::tempdir().unwrap();
        let chosen = mdm_tools::chosen::choose(data.path(), Helper::GalleryDl, &exe);
        if mdm_tools::gallery::too_old(&v) {
            assert!(
                matches!(chosen, Err(mdm_tools::Error::TooOld { .. })),
                "{v}: {chosen:?}"
            );
            assert_eq!(mdm_tools::gallery::installed(data.path()), None);
            rows.push(format!("{v}: too old, refused when chosen"));
            continue;
        }
        let tool = chosen.unwrap_or_else(|e| panic!("{v}: {e}"));
        assert_eq!(
            mdm_tools::gallery::installed(data.path()),
            Some(tool.clone())
        );
        let picture = lab.url(&format!("/gen/64K/picture-{}.jpg", v.replace('.', "-")));
        assert!(
            mdm_tools::gallery::handles(&tool, &picture).unwrap_or_else(|e| panic!("{v}: {e}")),
            "{v} reads a picture's address"
        );
        assert!(
            !mdm_tools::gallery::handles(&tool, &lab.url("/page"))
                .unwrap_or_else(|e| panic!("{v}: {e}")),
            "{v} says a page isn't one of its"
        );
        lab.clear_log();
        let folder = data.path().join("gallery");
        let job = GalleryJob::start(
            &tool,
            &picture,
            &folder,
            &Ask::default(),
            &Default::default(),
            None,
        )
        .unwrap_or_else(|e| panic!("{v}: {e}"));
        let deadline = Instant::now() + Duration::from_secs(90);
        let p = loop {
            let p = job.progress();
            if p.state != State::Running {
                break p;
            }
            assert!(Instant::now() < deadline, "{v}: still running");
            std::thread::sleep(Duration::from_millis(200));
        };
        assert!(
            matches!(&p.state, State::Done(f) if *f == folder),
            "{v}: {:?} {:?}",
            p.state,
            p.events
        );
        let saved: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(saved.len(), 1, "{v}: {saved:?}");
        assert_eq!(saved[0].metadata().unwrap().len(), 64 * 1024, "{v}");
        let log = lab.log();
        let get = log
            .iter()
            .find(|e| e.path.starts_with("/gen/64K/"))
            .unwrap_or_else(|| panic!("{v}: no request"));
        assert_eq!(get.user_agent.as_deref(), Some(USER_AGENT), "{v}");
        rows.push(format!(
            "{v}: chosen, reads and refuses addresses, saved {} with MDM's User-Agent",
            saved[0].file_name().to_string_lossy()
        ));
        // #769: with MDM's engine, gallery-dl lists the file (1.26 and later) and the engine
        // saves it under gallery-dl's name; gallery-dl then finds it there. An older one
        // downloads it itself, as before.
        lab.clear_log();
        let engine = mdm_tools::streams::Engine {
            handle: rt.handle().clone(),
            opts: Default::default(),
        };
        let folder2 = data.path().join("gallery2");
        let job = GalleryJob::start(
            &tool,
            &picture,
            &folder2,
            &Ask::default(),
            &Default::default(),
            Some(engine),
        )
        .unwrap_or_else(|e| panic!("{v}: {e}"));
        let deadline = Instant::now() + Duration::from_secs(90);
        let p = loop {
            let p = job.progress();
            if p.state != State::Running {
                break p;
            }
            assert!(Instant::now() < deadline, "{v}: still running");
            std::thread::sleep(Duration::from_millis(200));
        };
        assert!(matches!(&p.state, State::Done(_)), "{v}: {:?}", p.events);
        let names = |d: &std::path::Path| {
            let mut n: Vec<String> = std::fs::read_dir(d)
                .unwrap()
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            n.sort();
            n
        };
        assert_eq!(
            names(&folder2),
            names(&folder),
            "{v}: gallery-dl's own name"
        );
        let events: Vec<&str> = p.events.iter().map(|(_, e)| e.as_str()).collect();
        let by_engine = mdm_tools::gallery::Accepts::of(&v).print;
        assert_eq!(
            events.iter().any(|e| e.starts_with("MDM saved 1 files")),
            by_engine,
            "{v}: {events:?}"
        );
        if by_engine {
            let log = lab.log();
            let get = log
                .iter()
                .find(|e| e.path.starts_with("/gen/64K/"))
                .unwrap_or_else(|| panic!("{v}: no request"));
            assert_eq!(get.user_agent.as_deref(), Some(USER_AGENT), "{v}");
            rows.push(format!("{v}: listed, saved by MDM's engine"));
        }
    }
    for r in rows {
        println!("{r}");
    }
}
