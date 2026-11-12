//! The download list's selection, as a Windows list view keeps it: a set of selected rows, the
//! focused row (the caret: the dotted outline, where the keyboard moves from) and the anchor
//! (where a Shift range starts). Kept by download id so it survives sorting and new rows.

use std::collections::HashSet;

/// What a click or key asked for, by row index in the list as shown.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// Click / arrow keys: only this row.
    Only(usize),
    /// Ctrl+click, Ctrl+Space: this row in or out; the anchor moves to it.
    Toggle(usize),
    /// Shift+click / Shift+arrows: anchor to this row (`add`: Ctrl too, the rest stays).
    Range { to: usize, add: bool },
    /// Ctrl+arrows: the focus moves, the selection stays.
    Focus(usize),
    /// A selection box being dragged: the rows it touches now (`add`: Ctrl held, the selection
    /// from before the box stays).
    Rows { rows: Vec<usize>, add: bool },
    /// The selection box was let go.
    BoxDone,
    /// Ctrl+A.
    All,
    /// A click on blank space.
    Clear,
}

#[derive(Default)]
pub struct Selection {
    /// Selected downloads, in the order they were selected.
    ids: Vec<u64>,
    focus: Option<u64>,
    anchor: Option<u64>,
    /// While a selection box is dragged: the selection it adds to.
    boxing: Option<Vec<u64>>,
}

impl Selection {
    /// `rows`: the ids of the list's rows as shown.
    pub fn apply(&mut self, pick: Pick, rows: &[u64]) {
        let at = |i: usize| rows.get(i).copied();
        match pick {
            Pick::Only(i) => {
                if let Some(id) = at(i) {
                    self.ids = vec![id];
                    self.focus = Some(id);
                    self.anchor = Some(id);
                }
            }
            Pick::Toggle(i) => {
                if let Some(id) = at(i) {
                    if self.contains(id) {
                        self.ids.retain(|s| *s != id);
                    } else {
                        self.ids.push(id);
                    }
                    self.focus = Some(id);
                    self.anchor = Some(id);
                }
            }
            Pick::Range { to, add } => {
                let Some(id) = at(to) else { return };
                let from = self
                    .anchor
                    .and_then(|a| rows.iter().position(|r| *r == a))
                    .unwrap_or(to);
                let (lo, hi) = (from.min(to), from.max(to));
                if !add {
                    self.ids.clear();
                }
                let mut chosen: HashSet<u64> = self.ids.iter().copied().collect();
                for r in &rows[lo..=hi] {
                    if chosen.insert(*r) {
                        self.ids.push(*r);
                    }
                }
                self.focus = Some(id);
                self.anchor.get_or_insert(id);
            }
            Pick::Focus(i) => {
                if let Some(id) = at(i) {
                    self.focus = Some(id);
                }
            }
            Pick::Rows { rows: picked, add } => {
                let base = self
                    .boxing
                    .get_or_insert_with(|| if add { self.ids.clone() } else { Vec::new() });
                self.ids = base.clone();
                let mut chosen: HashSet<u64> = self.ids.iter().copied().collect();
                for id in picked.into_iter().filter_map(at) {
                    if chosen.insert(id) {
                        self.ids.push(id);
                    }
                }
                if let Some(last) = self.ids.last().copied() {
                    self.focus = Some(last);
                    self.anchor = Some(last);
                }
            }
            Pick::All => {
                self.ids = rows.to_vec();
                if self.focus.is_none_or(|f| !rows.contains(&f)) {
                    self.focus = rows.first().copied();
                }
            }
            Pick::BoxDone => self.boxing = None,
            Pick::Clear => self.ids.clear(),
        }
    }

    /// Just this download (a new download, a search match, a right-click outside the selection).
    pub fn set(&mut self, id: u64) {
        self.ids = vec![id];
        self.focus = Some(id);
        self.anchor = Some(id);
    }

    /// Just these downloads, the first focused (a group's summary's Show in list, #766).
    pub fn set_many(&mut self, ids: &[u64]) {
        self.ids = ids.to_vec();
        self.focus = ids.first().copied();
        self.anchor = self.focus;
    }

    /// The list as shown now (another view, a filter, a sort): what isn't shown stops being
    /// selected, as when Explorer changes folder, so no command acts on a download out of sight;
    /// the rest is kept in list order.
    pub fn retain(&mut self, rows: &[u64]) {
        if self.boxing.is_some() {
            return;
        }
        let chosen: HashSet<u64> = self.ids.iter().copied().collect();
        self.ids = rows
            .iter()
            .copied()
            .filter(|r| chosen.contains(r))
            .collect();
        let shown = |id: &u64| rows.contains(id);
        if !self.focus.as_ref().is_some_and(shown) {
            self.focus = None;
        }
        if !self.anchor.as_ref().is_some_and(shown) {
            self.anchor = None;
        }
    }

    /// A download left the list (deleted).
    pub fn forget(&mut self, id: u64) {
        self.ids.retain(|s| *s != id);
        if self.focus == Some(id) {
            self.focus = None;
        }
        if self.anchor == Some(id) {
            self.anchor = None;
        }
    }

    /// Rows were taken off the list (`before` → `after`, as shown): if nothing selected is
    /// left, the row after the first one gone is selected (the one before it at the end).
    pub fn after_removal(&mut self, before: &[u64], after: &[u64]) {
        if !self.ids.is_empty() {
            return;
        }
        let Some(gone) = before.iter().position(|id| !after.contains(id)) else {
            return;
        };
        let left = |id: &&u64| after.contains(id);
        let next = before[gone..]
            .iter()
            .find(left)
            .or_else(|| before[..gone].iter().rev().find(left));
        if let Some(id) = next {
            self.set(*id);
        }
    }

    pub fn contains(&self, id: u64) -> bool {
        self.ids.contains(&id)
    }

    /// The download single-download commands act on (Properties, Open…): the selected one when
    /// there is exactly one.
    pub fn one(&self) -> Option<u64> {
        match self.ids.as_slice() {
            [id] => Some(*id),
            _ => None,
        }
    }

    pub fn focus(&self) -> Option<u64> {
        self.focus
    }

    /// Every selected download, in list order (`rows`); ids not shown come last.
    #[cfg(test)]
    fn ids_in(&self, rows: &[u64]) -> Vec<u64> {
        let chosen: HashSet<u64> = self.ids.iter().copied().collect();
        let shown: HashSet<u64> = rows.iter().copied().collect();
        let mut out: Vec<u64> = rows
            .iter()
            .copied()
            .filter(|r| chosen.contains(r))
            .collect();
        out.extend(self.ids.iter().filter(|s| !shown.contains(s)));
        out
    }

    pub fn ids(&self) -> &[u64] {
        &self.ids
    }

    /// Selected rows (by index in `rows`) and the focused row, for painting.
    pub fn rows(&self, rows: &[u64]) -> (Vec<bool>, Option<usize>) {
        let chosen: HashSet<u64> = self.ids.iter().copied().collect();
        (
            rows.iter().map(|r| chosen.contains(r)).collect(),
            self.focus.and_then(|f| rows.iter().position(|r| *r == f)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_selects_the_row_after() {
        let mut s = Selection::default();
        s.set(3);
        s.apply(Pick::Range { to: 3, add: false }, &[1, 2, 3, 4, 5, 6]);
        // Rows 3 and 4 deleted: 5 is selected.
        for id in s.ids().to_vec() {
            s.forget(id);
        }
        s.after_removal(&[1, 2, 3, 4, 5, 6], &[1, 2, 5, 6]);
        assert_eq!(s.ids(), &[5]);
        // The last row deleted: the one before it.
        s.forget(6);
        s.set(6);
        s.forget(6);
        s.after_removal(&[1, 2, 5, 6], &[1, 2, 5]);
        assert_eq!((s.ids(), s.focus()), (&[5][..], Some(5)));
        // Something still selected: left alone.
        s.after_removal(&[1, 2, 5], &[1, 5]);
        assert_eq!(s.ids(), &[5]);
    }

    const ROWS: [u64; 6] = [10, 11, 12, 13, 14, 15];

    #[test]
    fn click_ctrl_click_and_shift_click_select_like_windows() {
        let mut s = Selection::default();
        s.apply(Pick::Only(1), &ROWS);
        assert_eq!(s.ids(), [11]);
        s.apply(Pick::Toggle(3), &ROWS);
        assert_eq!(s.ids_in(&ROWS), [11, 13]);
        // Shift from the anchor (13, the last Ctrl+click) replaces the selection.
        s.apply(Pick::Range { to: 5, add: false }, &ROWS);
        assert_eq!(s.ids_in(&ROWS), [13, 14, 15]);
        // The anchor stays: Shift again goes the other way from it.
        s.apply(Pick::Range { to: 2, add: false }, &ROWS);
        assert_eq!(s.ids_in(&ROWS), [12, 13]);
        assert_eq!(s.focus(), Some(12));
        // Ctrl+Shift adds the range.
        s.apply(Pick::Only(0), &ROWS);
        s.apply(Pick::Toggle(4), &ROWS);
        s.apply(Pick::Range { to: 5, add: true }, &ROWS);
        assert_eq!(s.ids_in(&ROWS), [10, 14, 15]);
        // Ctrl+click on a selected row takes it out.
        s.apply(Pick::Toggle(0), &ROWS);
        assert_eq!(s.ids_in(&ROWS), [14, 15]);
    }

    #[test]
    fn select_all_focus_and_one() {
        let mut s = Selection::default();
        s.apply(Pick::All, &ROWS);
        assert_eq!(s.ids_in(&ROWS), ROWS);
        assert_eq!(s.focus(), Some(10));
        assert_eq!(s.one(), None, "several selected: no single download");
        s.apply(Pick::Focus(2), &ROWS);
        assert_eq!(s.ids().len(), 6, "Ctrl+arrows only move the focus");
        s.apply(Pick::Toggle(2), &ROWS);
        s.retain(&[15, 12, 11, 10]);
        assert_eq!(s.ids(), [15, 11, 10], "list order, hidden ones dropped");
        s.forget(11);
        assert!(!s.contains(11));
        s.apply(Pick::Only(4), &ROWS);
        assert_eq!(s.one(), Some(14));
        s.apply(Pick::Clear, &ROWS);
        assert!(s.ids().is_empty());
    }

    #[test]
    fn a_box_selects_its_rows_and_shift_with_no_anchor_starts_there() {
        let mut s = Selection::default();
        s.apply(Pick::Range { to: 3, add: false }, &ROWS);
        assert_eq!(s.ids(), [13]);
        s.apply(
            Pick::Rows {
                rows: vec![0, 1],
                add: true,
            },
            &ROWS,
        );
        assert_eq!(s.ids_in(&ROWS), [10, 11, 13]);
        assert_eq!(s.rows(&ROWS).0, [true, true, false, true, false, false]);
        // The box shrinks: rows it left go, the selection from before it stays.
        s.apply(
            Pick::Rows {
                rows: vec![0],
                add: true,
            },
            &ROWS,
        );
        assert_eq!(s.ids_in(&ROWS), [10, 13]);
        s.apply(Pick::BoxDone, &ROWS);
        // A new box without Ctrl starts from nothing.
        s.apply(
            Pick::Rows {
                rows: vec![4],
                add: false,
            },
            &ROWS,
        );
        assert_eq!(s.ids(), [14]);
    }
}
