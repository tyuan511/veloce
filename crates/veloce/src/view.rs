//! Pure geometry and state for the three Finder view modes (list, icons, columns).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    List,
    Icons,
    Columns,
}

// ---- icon grid -------------------------------------------------------------

pub const ICON_CELL_W: f32 = 104.0;
pub const ICON_CELL_H: f32 = 104.0;
pub const GRID_PADDING: f32 = 8.0;

/// How many icon cells fit across `width` (always at least one).
pub fn grid_columns(width: f32) -> usize {
    let usable = (width - GRID_PADDING * 2.0).max(0.0);
    ((usable / ICON_CELL_W).floor() as usize).max(1)
}

/// Rows needed to lay `len` items out in `cols` columns.
pub fn grid_rows(len: usize, cols: usize) -> usize {
    len.div_ceil(cols.max(1))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridMove {
    Left,
    Right,
    Up,
    Down,
}

/// Cursor after an arrow key in the icon grid, clamped to the items that exist.
/// Down from a short last row lands on the last item, as in Finder.
pub fn grid_move(cursor: usize, len: usize, cols: usize, direction: GridMove) -> usize {
    if len == 0 {
        return 0;
    }
    let cols = cols.max(1);
    let cursor = cursor.min(len - 1);
    match direction {
        GridMove::Left => cursor.saturating_sub(1),
        GridMove::Right => (cursor + 1).min(len - 1),
        GridMove::Up => {
            if cursor >= cols {
                cursor - cols
            } else {
                cursor
            }
        }
        GridMove::Down => {
            if cursor + cols < len {
                cursor + cols
            } else if cursor / cols < (len - 1) / cols {
                len - 1
            } else {
                cursor
            }
        }
    }
}

/// Width available to one pane: the window minus the sidebar and the inspector,
/// shared between the panes that are showing.
pub fn pane_width(window: f32, sidebar: f32, inspector: f32, split: bool) -> f32 {
    let usable = (window - sidebar - inspector).max(0.0);
    if split { usable / 2.0 } else { usable }
}

// ---- columns ---------------------------------------------------------------

pub const COLUMN_WIDTH: f32 = 220.0;
pub const MAX_COLUMN_DIRS: usize = 3;

/// Directories to show as columns, outermost first, ending with `current`.
/// Only the deepest `max` are kept; the rest scroll out of view like Finder's.
pub fn column_dirs(current: &Path, max: usize) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = current.ancestors().map(Path::to_path_buf).collect();
    dirs.reverse();
    let keep = max.max(1);
    if dirs.len() > keep {
        dirs.drain(..dirs.len() - keep);
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_always_has_at_least_one_column() {
        assert_eq!(grid_columns(0.0), 1);
        assert_eq!(grid_columns(50.0), 1);
    }

    #[test]
    fn grid_columns_fit_cell_width_after_padding() {
        let width = GRID_PADDING * 2.0 + ICON_CELL_W * 5.0;
        assert_eq!(grid_columns(width), 5);
        assert_eq!(grid_columns(width - 1.0), 4);
    }

    #[test]
    fn grid_rows_round_up() {
        assert_eq!(grid_rows(0, 4), 0);
        assert_eq!(grid_rows(8, 4), 2);
        assert_eq!(grid_rows(9, 4), 3);
        assert_eq!(grid_rows(5, 0), 5, "zero columns is treated as one");
    }

    #[test]
    fn grid_left_and_right_clamp_at_the_ends() {
        assert_eq!(grid_move(0, 10, 4, GridMove::Left), 0);
        assert_eq!(grid_move(9, 10, 4, GridMove::Right), 9);
        assert_eq!(
            grid_move(3, 10, 4, GridMove::Right),
            4,
            "wraps to the next row"
        );
    }

    #[test]
    fn grid_up_and_down_move_by_a_row() {
        assert_eq!(grid_move(5, 10, 4, GridMove::Up), 1);
        assert_eq!(grid_move(1, 10, 4, GridMove::Down), 5);
        assert_eq!(grid_move(2, 10, 4, GridMove::Up), 2, "top row stays");
    }

    #[test]
    fn grid_down_into_a_short_last_row_lands_on_the_last_item() {
        // 10 items, 4 columns: the last row only has indices 8 and 9.
        assert_eq!(grid_move(3, 10, 4, GridMove::Down), 7);
        assert_eq!(grid_move(7, 10, 4, GridMove::Down), 9);
        assert_eq!(
            grid_move(9, 10, 4, GridMove::Down),
            9,
            "already on the last row"
        );
    }

    #[test]
    fn grid_move_survives_empty_lists_and_stale_cursors() {
        assert_eq!(grid_move(5, 0, 4, GridMove::Down), 0);
        assert_eq!(grid_move(99, 3, 4, GridMove::Left), 1);
    }

    #[test]
    fn pane_width_splits_what_the_sidebar_and_inspector_leave() {
        assert_eq!(pane_width(1000.0, 200.0, 0.0, false), 800.0);
        assert_eq!(pane_width(1000.0, 200.0, 200.0, true), 300.0);
        assert_eq!(pane_width(100.0, 200.0, 0.0, false), 0.0);
    }

    #[test]
    fn column_dirs_end_at_the_current_folder_and_keep_the_deepest() {
        let dirs = column_dirs(Path::new("/a/b/c/d"), 3);
        assert_eq!(
            dirs,
            [
                PathBuf::from("/a/b"),
                PathBuf::from("/a/b/c"),
                PathBuf::from("/a/b/c/d")
            ]
        );
    }

    #[test]
    fn column_dirs_of_a_shallow_path_include_the_root() {
        assert_eq!(
            column_dirs(Path::new("/a"), 3),
            [PathBuf::from("/"), PathBuf::from("/a")]
        );
        assert_eq!(column_dirs(Path::new("/"), 3), [PathBuf::from("/")]);
    }
}
