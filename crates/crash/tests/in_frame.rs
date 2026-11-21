//! The end-to-end test's crashy keeps a copy of the app's handler in front of crash-handler's
//! (#1239): the app keeps its `unsafe` code in its own unix.rs (CLAUDE.md, rule 3), so the two
//! can't share one file. This checks the copy is the same, so the crashes crashy reports go
//! through what MDM runs. (Text only: it runs on every system.)

const BEGIN: &str = "// ---- The handler in front of crash-handler's (#1239).";
const END: &str = "// ---- End of the handler in front of crash-handler's.";

/// From the begin mark to the end mark, both included, without carriage returns (a checkout
/// with Windows line ends).
fn handler(source: &str) -> Option<String> {
    let from = source.find(BEGIN)?;
    let to = from + source[from..].find(END)? + END.len();
    Some(source[from..to].replace('\r', ""))
}

#[test]
fn crashys_handler_is_the_apps() {
    let app = handler(include_str!("../../app/src/unix.rs")).expect("the app's handler, marked");
    let crashy =
        handler(include_str!("../examples/crashy.rs")).expect("crashy's copy of it, marked");
    assert!(app.contains("pub fn keep_handler_in_frame()"), "{app}");
    assert!(
        app == crashy,
        "examples/crashy.rs's copy of the handler differs from crates/app/src/unix.rs's: copy \
         the app's, from `{BEGIN}` to `{END}`"
    );
}

#[test]
fn a_handler_is_found_between_its_marks() {
    let src = format!("a\r\n{BEGIN}\r\nfn x() {{}}\r\n{END}\r\nb");
    assert_eq!(
        handler(&src).as_deref(),
        Some(format!("{BEGIN}\nfn x() {{}}\n{END}").as_str())
    );
    assert_eq!(handler("no marks"), None);
}
