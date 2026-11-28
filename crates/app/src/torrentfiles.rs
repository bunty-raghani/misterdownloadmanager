//! File list for any torrent, including ones no longer held in the torrent session.
use std::path::PathBuf;

use crate::{
    dialogs::{self, Outcome},
    model::{Item, Status},
    winui::{self, ButtonKind, LIGHT_BG, r},
};

pub struct TorrentFiles {
    pub id: u64,
    rows: Vec<(usize, mdm_torrent::FileEntry, bool, u64)>,
    base: PathBuf,
    complete: bool,
    pending: Option<(Vec<usize>, bool, u64)>,
    /// The torrent's name, and how many files and bytes it holds (the window's first line).
    heading: String,
}

/// Rows the list shows at once.
const SHOWN: usize = 15;
/// The progress window's table sits at y 273; this window's starts near the top.
const TABLE_DY: f32 = 36.0 - 273.0;

impl TorrentFiles {
    pub fn new(item: &Item, host: &crate::torrents::Host) -> Option<Self> {
        let spec = item.torrent.as_ref()?;
        // As last read: the window's thread never waits on librqbit (#693).
        let session = host.get().and_then(|t| {
            let (files, chosen, _) = t.seen_files(&spec.hash)?;
            let file_done = t
                .seen(&spec.hash)
                .map(|stats| stats.file_done)
                .unwrap_or_default();
            Some((files, chosen, file_done))
        });
        let (files, chosen, known_done) = if let Some((files, chosen, file_done)) = session {
            (files, chosen, Some(file_done))
        } else {
            let bytes = std::fs::read(crate::torrents::torrent_file(&spec.hash)?).ok()?;
            // Out of the session, what its pieces say: a file ticked later is made at its full
            // length at once, so its length on disk says nothing of what came.
            let map = crate::torrents::state_dir()
                .map(|d| mdm_torrent::piece_map_file(&d.join("session"), &spec.hash));
            (
                mdm_torrent::files_from_bytes(&bytes).ok()?,
                spec.files.clone(),
                map_done(&bytes, map.as_deref()),
            )
        };
        // `folder` already holds a torrent of several files' own folder (Meta::folder_in).
        let root = spec.folder.clone();
        let complete = item.status == Status::Complete;
        let rows: Vec<_> = files
            .into_iter()
            .enumerate()
            .filter(|(_, f)| !f.padding)
            .map(|(i, f)| {
                let chosen = chosen.as_ref().is_none_or(|v| v.contains(&i));
                let done = if complete {
                    f.size
                } else if let Some(file_done) = &known_done {
                    file_done.get(i).copied().unwrap_or(0).min(f.size)
                } else {
                    // No piece map: the session never checked it, or it left the session
                    // finished (librqbit removes its map with it), when its files were whole.
                    std::fs::metadata(root.join(&f.path))
                        .map(|m| m.len().min(f.size))
                        .unwrap_or(0)
                };
                (i, f, chosen, done)
            })
            .collect();
        let total: u64 = rows
            .iter()
            .map(|r: &(usize, mdm_torrent::FileEntry, bool, u64)| r.1.size)
            .sum();
        let heading = format!(
            "{}  ({}, {})",
            item.file_name,
            crate::i18n::tn(rows.len() as u64, "{n} file", "{n} files"),
            crate::model::size(total)
        );
        Some(Self {
            id: item.id,
            heading,
            rows,
            base: root,
            complete,
            pending: None,
        })
    }

    pub fn take_selection(&mut self) -> Option<(Vec<usize>, bool, u64)> {
        self.pending.take()
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<Option<(Vec<usize>, bool, u64)>> {
        let mut changed = None;
        let outcome = dialogs::window(
            ctx,
            &format!("torrent-files-{}", self.id),
            // i18n: title of the separate window listing a torrent's files
            "Torrent files",
            [548.0, 430.0],
            false,
            LIGHT_BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let (head_rect, head_cut) = winui::text_fit(
                    &p,
                    o.x + 18.0,
                    o.y + 24.0,
                    &self.heading,
                    winui::TEXT(),
                    o.x + 528.0,
                );
                winui::copyable(
                    ui,
                    head_rect,
                    "torrent-files-heading",
                    &self.heading,
                    head_cut,
                );
                let table_o = o + egui::vec2(0.0, TABLE_DY);
                let body_top = table_o.y + 299.0;
                let body_bottom = body_top + crate::progress::table_body_height(SHOWN);
                let mut all = self.rows.iter().all(|r| r.2);
                // i18n: check or uncheck all file rows in the torrent file list
                let response = winui::checkbox(
                    ui,
                    egui::pos2(o.x + 20.0, body_bottom + 12.0),
                    &mut all,
                    crate::i18n::t("Select all"),
                    true,
                );
                if response.changed() {
                    for row in &mut self.rows {
                        row.2 = all;
                    }
                    if !all && let Some(last) = self.rows.last_mut() {
                        last.2 = true;
                    }
                }
                let table_rows: Vec<Vec<String>> = self
                    .rows
                    .iter()
                    .map(|(_, f, on, done)| {
                        vec![
                            f.path.display().to_string(),
                            crate::model::size(f.size),
                            if !on {
                                // i18n: status of a torrent file the user chose not to download
                                crate::i18n::t("Skipped").into()
                            } else {
                                crate::i18n::percent(
                                    if f.size == 0 {
                                        1.0
                                    } else {
                                        *done as f64 / f.size as f64
                                    },
                                    1,
                                )
                            },
                        ]
                    })
                    .collect();
                let ticks: Vec<bool> = self.rows.iter().map(|r| r.2).collect();
                if let Some(row) = crate::progress::files_table(
                    ui,
                    &p,
                    table_o,
                    "torrent-files",
                    &table_rows,
                    &ticks,
                    SHOWN,
                ) {
                    self.rows[row].2 = !self.rows[row].2;
                    if !self.rows.iter().any(|row| row.2) {
                        self.rows[row].2 = true;
                    }
                }
                let double = ui.input(|i| {
                    i.pointer
                        .button_double_clicked(egui::PointerButton::Primary)
                        .then(|| i.pointer.interact_pos())
                        .flatten()
                });
                if self.complete
                    && let Some(pos) = double
                {
                    let viewport = ui.ctx().viewport_id();
                    let table_id = egui::Id::unique(("torrent-table", "torrent-files", viewport));
                    let first = ui
                        .ctx()
                        .data(|d| d.get_temp::<usize>(table_id))
                        .unwrap_or(0);
                    let row = ((pos.y - body_top) / 17.0).floor() as isize + first as isize;
                    // Past the row's check box and short of the scroll bar.
                    let on_a_name = pos.x >= o.x + 42.0 && pos.x < o.x + 511.0;
                    if on_a_name
                        && pos.y >= body_top
                        && pos.y < body_bottom
                        && row >= 0
                        && let Some((_, file, _, _)) = self.rows.get(row as usize)
                    {
                        dialogs::open_file(&self.base.join(&file.path));
                    }
                }
                let (open_at, folder_at, close_at) = (18.0, 380.0, 458.0);
                let open = winui::button(
                    ui,
                    r(o, open_at, 407.0, 75.0, 22.0),
                    "Open",
                    // Nothing says which of several files: those open by a double click.
                    if self.complete && self.rows.len() == 1 {
                        ButtonKind::Normal
                    } else {
                        ButtonKind::Disabled
                    },
                    false,
                    None,
                )
                .clicked();
                if open
                    && self.complete
                    && self.rows.len() == 1
                    && let Some((_, f, _, _)) = self.rows.first()
                {
                    dialogs::open_file(&self.base.join(&f.path));
                }
                if winui::button(
                    ui,
                    r(o, folder_at, 407.0, 75.0, 22.0),
                    // i18n: button that opens the torrent's containing folder in the file manager
                    "Open folder",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked()
                {
                    dialogs::show_in_folder(&self.base);
                }
                let close = winui::button(
                    ui,
                    r(o, close_at, 407.0, 72.0, 22.0),
                    // i18n: button that closes the torrent file list window
                    "Close",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked();
                if winui::escape_pressed(ui) || close {
                    Outcome::Closed
                } else if response.changed()
                    || !ticks.iter().zip(&self.rows).all(|(a, b)| *a == b.2)
                {
                    let chosen = self.rows.iter().filter(|r| r.2);
                    let files = chosen.clone().map(|r| r.0).collect::<Vec<_>>();
                    let all = files.len() == self.rows.len();
                    let size = chosen.map(|r| r.1.size).sum();
                    changed = Some((files, all, size));
                    Outcome::Open
                } else {
                    Outcome::Open
                }
            },
        );
        match outcome {
            Outcome::Open => {
                self.pending = changed;
                Outcome::Open
            }
            Outcome::Closed => Outcome::Closed,
            Outcome::Done(()) => Outcome::Done(None),
        }
    }
}

/// What each file has, from the torrent's saved piece map at `map` (`mdm_torrent::piece_map_file`):
/// None without a map, or with one that isn't this torrent's.
fn map_done(torrent: &[u8], map: Option<&std::path::Path>) -> Option<Vec<u64>> {
    mdm_torrent::file_done_from_map(torrent, &std::fs::read(map?).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A .torrent of `files` (name, length) in pieces of `piece` bytes, bencoded by hand.
    fn torrent_of(files: &[(&str, u64)], piece: u64) -> Vec<u8> {
        let total: u64 = files.iter().map(|f| f.1).sum();
        let mut list = Vec::new();
        for (name, len) in files {
            list.extend(format!("d6:lengthi{len}e4:pathl{}:{name}ee", name.len()).bytes());
        }
        let mut out = b"d4:infod5:filesl".to_vec();
        out.extend(list);
        out.extend(format!("e4:name3:set12:piece lengthi{piece}e6:pieces").bytes());
        let hashes = total.div_ceil(piece) * 20;
        out.extend(format!("{hashes}:").bytes());
        out.extend(std::iter::repeat_n(0u8, hashes as usize));
        out.extend(b"ee");
        out
    }

    /// #1624: a file ticked later is made at its full length at once; out of the session, its
    /// pieces say what came, not its length.
    #[test]
    fn a_torrent_file_made_at_full_length_shows_only_its_pieces() {
        let piece = 16 * 1024;
        let torrent = torrent_of(&[("a.bin", 64 * 1024), ("b.bin", 1024 * 1024)], piece);
        let tmp = tempfile::tempdir().unwrap();
        // The sparse b.bin at its full length, as the engine makes it.
        let b = std::fs::File::create(tmp.path().join("b.bin")).unwrap();
        b.set_len(1024 * 1024).unwrap();
        assert_eq!(
            std::fs::metadata(tmp.path().join("b.bin")).unwrap().len(),
            1024 * 1024
        );
        // 68 pieces: a's four (0-3) and one of b's (34).
        let mut bits = vec![0u8; 9];
        bits[0] = 0xf0;
        bits[4] = 0x20;
        let map = tmp.path().join("hash.bitv");
        std::fs::write(&map, &bits).unwrap();
        assert_eq!(map_done(&torrent, Some(&map)), Some(vec![64 * 1024, piece]));
    }

    /// No map (or one not of this torrent): nothing from it; the window falls back to what is on
    /// the disk.
    #[test]
    fn a_torrent_without_its_piece_map_says_nothing() {
        let torrent = torrent_of(&[("a.bin", 64 * 1024)], 16 * 1024);
        let tmp = tempfile::tempdir().unwrap();
        let map = tmp.path().join("hash.bitv");
        assert_eq!(map_done(&torrent, Some(&map)), None);
        assert_eq!(map_done(&torrent, None), None);
        std::fs::write(&map, []).unwrap();
        assert_eq!(map_done(&torrent, Some(&map)), None);
    }
}
