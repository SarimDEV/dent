//! The chat's list rows, derived from a session's items.
//!
//! The list lays out every row on screen each frame, and measures a row
//! again whenever its signature changes. A reply is one item but can be
//! long, so it's drawn a row per top-level block: a long reply then costs
//! only what's on screen, and as it streams only its last block is measured
//! again. Everything else about the chat (selection, Find, copying, folds)
//! still speaks of items; only the list and what scrolls it speak of rows,
//! through here.

/// One row of the chat's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Row {
    /// The item it draws; `None` for the working line after the last item.
    pub item: Option<usize>,
    /// For a reply drawn a block per row, which block; `None` for the
    /// whole item.
    pub part: Option<Part>,
}

/// Block `block` of an item's `of` top-level blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Part {
    pub block: usize,
    pub of: usize,
}

impl Row {
    /// The row its item starts on: what's drawn above an item (the
    /// working line, a turn's line) goes here.
    pub fn first(&self) -> bool {
        self.part.is_none_or(|p| p.block == 0)
    }

    /// The row its item ends on: what's drawn below an item (a turn's time
    /// and actions, the gap to the next item) goes here.
    pub fn last(&self) -> bool {
        self.part.is_none_or(|p| p.block + 1 == p.of)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rows {
    pub rows: Vec<Row>,
    /// Where each item's rows start.
    starts: Vec<usize>,
}

impl Rows {
    /// Rows for `len` items, where `parts(ix)` is how many rows item `ix`
    /// is drawn in (a reply's blocks; 1 for anything drawn whole), and a
    /// last row for the working line when `working` (and it has no item
    /// to sit above).
    pub fn derive(len: usize, working: bool, parts: impl Fn(usize) -> usize) -> Self {
        let mut rows = Vec::with_capacity(len + 1);
        let mut starts = Vec::with_capacity(len);
        for ix in 0..len {
            starts.push(rows.len());
            match parts(ix) {
                0 | 1 => rows.push(Row {
                    item: Some(ix),
                    part: None,
                }),
                of => rows.extend((0..of).map(|block| Row {
                    item: Some(ix),
                    part: Some(Part { block, of }),
                })),
            }
        }
        if working {
            rows.push(Row {
                item: None,
                part: None,
            });
        }
        Rows { rows, starts }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// How many items the rows are of.
    pub fn items(&self) -> usize {
        self.starts.len()
    }

    /// Where the items' rows end: the working line's row, if there is one.
    pub fn end_of_items(&self) -> usize {
        self.rows.len() - usize::from(self.rows.last().is_some_and(|r| r.item.is_none()))
    }

    /// The rows item `ix` is drawn in.
    pub fn of(&self, ix: usize) -> std::ops::Range<usize> {
        let start = self.starts.get(ix).copied().unwrap_or(self.end_of_items());
        let end = self
            .starts
            .get(ix + 1)
            .copied()
            .unwrap_or(self.end_of_items());
        start..end.max(start)
    }

    pub fn get(&self, row: usize) -> Option<Row> {
        self.rows.get(row).copied()
    }

    /// The row item `ix` starts on.
    pub fn of_item(&self, ix: usize) -> Option<usize> {
        self.starts.get(ix).copied()
    }

    /// The row holding block `block` of item `ix` (its only row, if it's
    /// drawn whole).
    pub fn of_block(&self, ix: usize, block: usize) -> Option<usize> {
        let start = self.of_item(ix)?;
        match self.rows.get(start)?.part {
            Some(p) => Some(start + block.min(p.of - 1)),
            None => Some(start),
        }
    }

    /// The item a row draws, for anything that speaks of items; the
    /// working line's row counts as the end.
    pub fn item_of(&self, row: usize) -> usize {
        match self.rows.get(row) {
            Some(Row { item: Some(ix), .. }) => *ix,
            _ => self.starts.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_is_a_row_per_block_and_the_rest_one_each() {
        // A message, a reply of three blocks, a tool group; then working.
        let r = Rows::derive(3, true, |ix| if ix == 1 { 3 } else { 1 });
        assert_eq!(r.len(), 6);
        assert_eq!(r.of_item(0), Some(0));
        assert_eq!(r.of_item(1), Some(1));
        assert_eq!(r.of_item(2), Some(4));
        assert_eq!(r.of_block(1, 2), Some(3));
        // Past its last block is its last row; a whole item is its row.
        assert_eq!(r.of_block(1, 9), Some(3));
        assert_eq!(r.of_block(2, 1), Some(4));
        assert_eq!(r.item_of(2), 1);
        // The working line's row is past every item.
        assert_eq!(r.get(5).and_then(|r| r.item), None);
        assert_eq!(r.item_of(5), 3);
        let firsts: Vec<bool> = r.rows.iter().map(Row::first).collect();
        let lasts: Vec<bool> = r.rows.iter().map(Row::last).collect();
        assert_eq!(firsts, [true, true, false, false, true, true]);
        assert_eq!(lasts, [true, false, false, true, true, true]);
    }

    #[test]
    fn a_reply_of_one_block_is_drawn_whole() {
        let r = Rows::derive(1, false, |_| 1);
        assert_eq!(
            r.rows,
            [Row {
                item: Some(0),
                part: None
            }]
        );
        // Nothing parsed yet counts as drawn whole too.
        assert_eq!(Rows::derive(1, false, |_| 0).rows, r.rows);
        assert_eq!(Rows::derive(0, false, |_| 1).len(), 0);
    }
}
