//! What a resumed download asked again of what it had kept (#123, #1034): the tests that resume
//! one check it request by request, not as a total. How many bytes come twice depends on timing
//! (a part split between two connections while its bytes are on their way), so a total of what
//! the server sent grows on a busy machine; what was on disk before the resume is asked again
//! only as each part's check (`Options::verify_overlap`), however busy the machine.

/// Bytes of `range` (a `Range` header, `bytes=a-b`; none: the whole file) that the saved
/// segments say were already on disk: outside every segment's part still to fetch.
pub fn on_disk(range: Option<&str>, saved: &mdm_engine::state::ResumeState) -> u64 {
    let (from, to) = match range.and_then(|r| r.strip_prefix("bytes=")?.split_once('-')) {
        Some((a, b)) => (
            a.parse::<u64>().unwrap(),
            b.parse::<u64>().map_or(saved.size, |b| b + 1),
        ),
        None => (0, saved.size),
    };
    let to_fetch: u64 = saved
        .segments
        .iter()
        .map(|g| to.min(g.end).saturating_sub(from.max(g.cursor)))
        .sum();
    (to - from) - to_fetch
}
