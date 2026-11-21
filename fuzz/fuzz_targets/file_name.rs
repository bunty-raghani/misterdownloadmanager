//! A downloaded file's name, from what a server and a page say of it (`mdm_engine::naming`:
//! `Content-Disposition`, the address, `Content-Type`, the page's title, the first bytes), and an
//! archive entry's name made a path under the extraction folder (`mdm_extract::safe::rel_path`).
//!
//! The input's lines are the `Content-Disposition` value, the address, the `Content-Type` and the
//! page's title; the whole input is also the file's first bytes and an archive entry's name.
#![no_main]

use std::path::Component;

use libfuzzer_sys::fuzz_target;
use mdm_engine::naming::{self, Answer};
use url::Url;

/// What every name MDM makes must be: one plain path component, valid on Windows (the strictest
/// system), so a server can't name a file outside the folder or a device.
fn check_name(name: &str) {
    assert!(!name.is_empty());
    assert!(name != "." && name != "..", "{name:?}");
    assert!(!name.contains(['/', '\\']), "separator in {name:?}");
    assert!(
        !name
            .chars()
            .any(|c| c.is_control() || r#"<>:"|?*"#.contains(c)),
        "a character Windows refuses in {name:?}"
    );
    assert!(!name.chars().any(naming::is_format_char), "{name:?}");
    assert!(
        !name.ends_with(['.', ' ']),
        "trailing dot or space in {name:?}"
    );
    assert!(!naming::is_windows_reserved(name), "device name {name:?}");
}

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let mut lines = text.split('\n');
    let disposition = lines.next().unwrap_or_default();
    let address = lines.next().unwrap_or_default();
    let content_type = lines.next();
    let title = lines.next();

    let _ = naming::disposition_name(disposition);
    check_name(&naming::sanitize(&text));
    let cleaned = naming::clean_title(&text);
    assert!(cleaned.chars().count() <= 150);
    let sniffed = naming::sniff(data);
    let _ = naming::means_nothing(&text);
    let _ = naming::is_script_name(&text);
    let _ = naming::type_extension(&text);

    let base = Url::parse("https://files.example.com/get/").unwrap();
    let url = Url::parse(address)
        .or_else(|_| base.join(address))
        .unwrap_or(base);
    check_name(&naming::file_name(Some(disposition), &url));
    check_name(&naming::file_name(None, &url));
    check_name(&naming::answer_name(
        &url,
        &Answer {
            disposition: None,
            content_type,
            sniffed,
            title,
        },
    ));
    let _ = naming::address_name(&url);
    let _ = naming::needs_sniff(&url, Some(disposition), content_type);

    // An archive entry: a relative path of plain, safe names, or refused.
    if let Ok(path) = mdm_extract::safe::rel_path(&text, 64) {
        assert!(path.components().count() <= 64);
        for c in path.components() {
            let Component::Normal(part) = c else {
                panic!("{path:?} is not a plain path");
            };
            check_name(&part.to_string_lossy());
        }
    }
});
