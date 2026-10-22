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
pub const ICON_CELL_H: f32 = 112.0;
pub const GRID_PADDING: f32 = 8.0;

/// How many icon cells fit across `width` (always at least one).
pub fn grid_columns(width: f32) -> usize {
    let usable = (width - GRID_PADDING * 2.0).max(0.0);
    ((usable / ICON_CELL_W).floor() as usize).max(1)
}

/// 估算图标名字的显示宽度（px，12px 字号）：宽字符按一个字宽，其余按半个。
fn name_width(c: char) -> f32 {
    if c.is_ascii() { 6.5 } else { 12.0 }
}

/// 两行放不下的名字做中间省略，保留开头和结尾（含扩展名），像访达那样。
/// 手动换行宽度未知，所以按估算宽度裁剪，宁可少显示一点也不让第二行被切掉。
pub fn middle_ellipsis(name: &str, budget: f32) -> String {
    let chars: Vec<char> = name.chars().collect();
    let total: f32 = chars.iter().map(|c| name_width(*c)).sum();
    if total <= budget {
        return name.to_string();
    }
    let ellipsis = 12.0;
    let avail = (budget - ellipsis).max(0.0);
    // 第一行满，第二行留给结尾。
    let head_budget = avail * 0.5;
    let tail_budget = avail - head_budget;
    let mut head = String::new();
    let mut used = 0.0;
    for c in &chars {
        let w = name_width(*c);
        if used + w > head_budget {
            break;
        }
        used += w;
        head.push(*c);
    }
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0.0;
    for c in chars.iter().rev() {
        let w = name_width(*c);
        if used + w > tail_budget {
            break;
        }
        used += w;
        tail.push(*c);
    }
    tail.reverse();
    format!("{head}…{}", tail.into_iter().collect::<String>())
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

// ---- resizable layout --------------------------------------------------------

/// Width of a draggable divider (the visible line is 1px inside it).
pub const HANDLE_WIDTH: f32 = 5.0;
pub const SIDEBAR_DEFAULT: f32 = 176.0;
pub const SIDEBAR_MIN: f32 = 140.0;
pub const SIDEBAR_MAX: f32 = 320.0;
pub const INSPECTOR_DEFAULT: f32 = 340.0;
pub const INSPECTOR_MIN: f32 = 240.0;
pub const INSPECTOR_MAX: f32 = 720.0;
/// Narrowest a file pane may get.
pub const PANE_MIN: f32 = 260.0;

/// The three dividers a user can drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divider {
    /// Between the sidebar and the panes.
    Sidebar,
    /// Between the two panes.
    Panes,
    /// Between the panes and the inspector.
    Inspector,
}

/// Window space taken by the inspector and its divider (nothing when closed).
fn inspector_block(inspector: f32, inspector_open: bool) -> f32 {
    if inspector_open {
        inspector + HANDLE_WIDTH
    } else {
        0.0
    }
}

/// Space the file panes can share: the window minus the sidebar, the inspector
/// and every divider.
pub fn panes_area(
    window: f32,
    sidebar: f32,
    inspector: f32,
    inspector_open: bool,
    split: bool,
) -> f32 {
    let handles = HANDLE_WIDTH + if split { HANDLE_WIDTH } else { 0.0 };
    (window - sidebar - inspector_block(inspector, inspector_open) - handles).max(0.0)
}

/// Keep the sidebar between its limits *and* leave the panes their minimum room.
pub fn clamp_sidebar(
    width: f32,
    window: f32,
    inspector: f32,
    inspector_open: bool,
    split: bool,
) -> f32 {
    let panes_min = PANE_MIN * if split { 2.0 } else { 1.0 };
    let handles = HANDLE_WIDTH + if split { HANDLE_WIDTH } else { 0.0 };
    let room = window - inspector_block(inspector, inspector_open) - handles - panes_min;
    width.clamp(SIDEBAR_MIN, SIDEBAR_MAX.min(room).max(SIDEBAR_MIN))
}

/// Keep the inspector between its limits and leave the panes their minimum room.
pub fn clamp_inspector(width: f32, window: f32, sidebar: f32, split: bool) -> f32 {
    let panes_min = PANE_MIN * if split { 2.0 } else { 1.0 };
    let handles = HANDLE_WIDTH * if split { 3.0 } else { 2.0 };
    let room = window - sidebar - handles - panes_min;
    width.clamp(INSPECTOR_MIN, INSPECTOR_MAX.min(room).max(INSPECTOR_MIN))
}

/// Left pane's share of the panes area, kept so both panes stay usable.
pub fn clamp_ratio(ratio: f32, area: f32) -> f32 {
    if area <= PANE_MIN * 2.0 {
        return 0.5;
    }
    let lowest = PANE_MIN / area;
    ratio.clamp(lowest, 1.0 - lowest)
}

/// Widths of the left and right file panes.
pub fn pane_widths(
    window: f32,
    sidebar: f32,
    inspector: f32,
    inspector_open: bool,
    split: bool,
    ratio: f32,
) -> (f32, f32) {
    let area = panes_area(window, sidebar, inspector, inspector_open, split);
    if !split {
        return (area, 0.0);
    }
    let left = (area * clamp_ratio(ratio, area)).round();
    (left, area - left)
}

/// Pointer x (window coordinates) → new sidebar width while dragging.
pub fn sidebar_from_pointer(
    pointer_x: f32,
    window: f32,
    inspector: f32,
    inspector_open: bool,
    split: bool,
) -> f32 {
    clamp_sidebar(
        pointer_x - HANDLE_WIDTH / 2.0,
        window,
        inspector,
        inspector_open,
        split,
    )
}

/// Pointer x → new inspector width while dragging its left edge.
pub fn inspector_from_pointer(pointer_x: f32, window: f32, sidebar: f32, split: bool) -> f32 {
    clamp_inspector(
        window - pointer_x - HANDLE_WIDTH / 2.0,
        window,
        sidebar,
        split,
    )
}

/// Pointer x → new left-pane share while dragging the divider between the panes.
pub fn ratio_from_pointer(pointer_x: f32, sidebar: f32, area: f32) -> f32 {
    if area <= 0.0 {
        return 0.5;
    }
    let left = pointer_x - sidebar - HANDLE_WIDTH - HANDLE_WIDTH / 2.0;
    clamp_ratio(left / area, area)
}

// ---- minimum widths --------------------------------------------------------

/// Narrowest the list may be laid out: icon, a readable name, and the three data
/// columns. A pane narrower than this scrolls sideways instead of letting the
/// columns pile onto each other.
pub const LIST_NAME_MIN: f32 = 180.0;
pub const LIST_ICON_AREA: f32 = 26.0;

/// Minimum content width of a view, given the fixed data-column widths. `None`
/// means the view adapts by itself and never needs a sideways scroll.
pub fn min_content_width(view: ViewMode, data_columns: f32) -> Option<f32> {
    match view {
        ViewMode::List => Some(LIST_ICON_AREA + LIST_NAME_MIN + data_columns),
        // The grid reflows into fewer columns; one cell is the floor.
        ViewMode::Icons => Some(ICON_CELL_W + GRID_PADDING * 2.0),
        // Columns lay out their own scrolling area (their width depends on how
        // many folders are open).
        ViewMode::Columns => None,
    }
}

/// Width of the columns browser: every open folder plus the detail column.
pub fn columns_content_width(folders: usize) -> f32 {
    folders as f32 * COLUMN_WIDTH + DETAIL_MIN
}

/// Narrowest the detail column (selected file or folder contents) may get.
pub const DETAIL_MIN: f32 = 240.0;

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
    #[test]
    fn short_names_are_left_alone() {
        assert_eq!(middle_ellipsis("a.txt", 150.0), "a.txt");
    }

    #[test]
    fn long_names_keep_both_ends_and_the_extension() {
        let out = middle_ellipsis("AutoResearch_可行性分析与实施建议报告.md", 150.0);
        assert!(out.starts_with("AutoRes"));
        assert!(out.ends_with(".md"));
        assert!(out.contains('…'));
    }

    #[test]
    fn zero_budget_does_not_panic() {
        assert_eq!(middle_ellipsis("abc", 0.0), "…");
        assert_eq!(middle_ellipsis("", 0.0), "");
    }

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
    fn panes_area_subtracts_every_divider_and_panel() {
        assert_eq!(panes_area(1000.0, 200.0, 0.0, false, false), 795.0);
        assert_eq!(panes_area(1000.0, 200.0, 300.0, true, false), 490.0);
        assert_eq!(panes_area(1000.0, 200.0, 300.0, true, true), 485.0);
        assert_eq!(panes_area(100.0, 200.0, 0.0, false, false), 0.0);
    }

    #[test]
    fn closed_inspector_takes_no_space_whatever_its_width() {
        assert_eq!(
            panes_area(1000.0, 200.0, 500.0, false, false),
            panes_area(1000.0, 200.0, 0.0, false, false)
        );
    }

    #[test]
    fn pane_widths_fill_the_area_exactly() {
        for ratio in [0.2, 0.5, 0.73] {
            let (left, right) = pane_widths(1400.0, 176.0, 340.0, true, true, ratio);
            let area = panes_area(1400.0, 176.0, 340.0, true, true);
            assert!((left + right - area).abs() < 0.01, "ratio {ratio}");
            assert!(left >= PANE_MIN && right >= PANE_MIN);
        }
    }

    #[test]
    fn a_single_pane_gets_all_the_room() {
        let (left, right) = pane_widths(1000.0, 200.0, 0.0, false, false, 0.5);
        assert_eq!(left, 795.0);
        assert_eq!(right, 0.0);
    }

    #[test]
    fn ratio_never_squeezes_a_pane_below_its_minimum() {
        let area = 1000.0;
        let low = clamp_ratio(0.01, area);
        let high = clamp_ratio(0.99, area);
        assert_eq!(low * area, PANE_MIN);
        assert_eq!((1.0 - high) * area, PANE_MIN);
        assert_eq!(clamp_ratio(0.5, area), 0.5);
    }

    #[test]
    fn a_tiny_area_falls_back_to_an_even_split() {
        assert_eq!(clamp_ratio(0.9, 300.0), 0.5);
        assert_eq!(ratio_from_pointer(10.0, 176.0, 0.0), 0.5);
    }

    #[test]
    fn dragging_the_sidebar_respects_its_limits_and_the_panes_minimum() {
        assert_eq!(
            sidebar_from_pointer(10.0, 1400.0, 340.0, true, false),
            SIDEBAR_MIN
        );
        assert_eq!(
            sidebar_from_pointer(900.0, 1400.0, 340.0, true, false),
            SIDEBAR_MAX
        );
        assert_eq!(
            sidebar_from_pointer(200.0, 1400.0, 340.0, true, false),
            197.5
        );
        // A narrow window caps the sidebar so the panes keep their room.
        let capped = sidebar_from_pointer(600.0, 800.0, 0.0, false, true);
        assert!(capped < SIDEBAR_MAX);
        assert!(capped >= SIDEBAR_MIN);
    }

    #[test]
    fn dragging_the_inspector_grows_it_leftwards() {
        let narrow = inspector_from_pointer(1100.0, 1400.0, 176.0, false);
        let wide = inspector_from_pointer(900.0, 1400.0, 176.0, false);
        assert!(wide > narrow);
        assert_eq!(
            inspector_from_pointer(1390.0, 1400.0, 176.0, false),
            INSPECTOR_MIN
        );
        assert_eq!(
            inspector_from_pointer(10.0, 3000.0, 176.0, false),
            INSPECTOR_MAX
        );
    }

    #[test]
    fn inspector_leaves_room_for_two_panes_when_split() {
        let single = inspector_from_pointer(300.0, 1000.0, 176.0, false);
        let split = inspector_from_pointer(300.0, 1000.0, 176.0, true);
        assert!(split <= single);
        assert!(split >= INSPECTOR_MIN);
    }

    #[test]
    fn dragging_between_the_panes_sets_the_left_share() {
        let sidebar = 176.0;
        let area = 1000.0;
        let middle = sidebar + HANDLE_WIDTH + area * 0.5 + HANDLE_WIDTH / 2.0;
        assert!((ratio_from_pointer(middle, sidebar, area) - 0.5).abs() < 0.001);
        assert!(ratio_from_pointer(0.0, sidebar, area) >= PANE_MIN / area);
        assert!(ratio_from_pointer(5000.0, sidebar, area) <= 1.0 - PANE_MIN / area);
    }

    #[test]
    fn layout_math_never_goes_negative_or_nan() {
        for window in [0.0, 100.0, 500.0] {
            let (l, r) = pane_widths(window, 176.0, 340.0, true, true, 0.5);
            assert!(l >= 0.0 && r >= 0.0 && l.is_finite() && r.is_finite());
        }
    }

    #[test]
    fn the_list_floor_covers_icon_name_and_every_data_column() {
        let min = min_content_width(ViewMode::List, 324.0).unwrap();
        assert_eq!(min, LIST_ICON_AREA + LIST_NAME_MIN + 324.0);
        assert!(min > 500.0, "wide enough that columns cannot overlap");
    }

    #[test]
    fn the_icon_grid_floor_is_one_cell() {
        let min = min_content_width(ViewMode::Icons, 0.0).unwrap();
        assert_eq!(min, ICON_CELL_W + GRID_PADDING * 2.0);
        assert_eq!(grid_columns(min), 1);
    }

    #[test]
    fn the_columns_view_sizes_itself_from_its_folders() {
        assert_eq!(min_content_width(ViewMode::Columns, 0.0), None);
        assert_eq!(columns_content_width(1), COLUMN_WIDTH + DETAIL_MIN);
        assert!(columns_content_width(3) > columns_content_width(2));
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
