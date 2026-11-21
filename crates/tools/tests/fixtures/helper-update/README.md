# A fake yt-dlp release

For `autoupdate`'s end-to-end test (crates/tools/src/autoupdate.rs): yt-dlp's file names for
each system, holding nothing but a line of text, and their `SHA2-256SUMS` signed by a throwaway
test key made for it (`test-key.asc`, its public half; the secret half was not kept). Never
yt-dlp's key: MDM accepts yt-dlp's real releases only with its pinned key (`keys/yt-dlp.asc`).
