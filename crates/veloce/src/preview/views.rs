//! Drawing the rich preview kinds: highlighted code, Markdown and tables.

use std::ops::Range;

use gpui::{
    AnyElement, FontStyle, FontWeight, HighlightStyle, StrikethroughStyle, StyledText,
    UnderlineStyle, div, prelude::*, px,
};

use super::{
    code::{self, Line},
    markdown::{Block, Inline, ListItem},
};
use crate::theme;

type Highlights = Vec<(Range<usize>, HighlightStyle)>;

/// Text and highlight ranges for one line of highlighted code.
pub(super) fn run_highlights(line: &Line) -> (String, Highlights) {
    let mut text = String::new();
    let mut ranges = Vec::new();
    for run in line {
        let start = text.len();
        text.push_str(&run.text);
        ranges.push((
            start..text.len(),
            HighlightStyle {
                color: Some(gpui::rgb(run.rgb).into()),
                font_weight: run.bold.then_some(FontWeight::BOLD),
                font_style: run.italic.then_some(FontStyle::Italic),
                ..Default::default()
            },
        ));
    }
    (text, ranges)
}

/// Text and highlight ranges for a Markdown inline run.
pub(super) fn inline_highlights(inline: &Inline) -> (String, Highlights) {
    let mut text = String::new();
    let mut ranges = Vec::new();
    for span in inline {
        let start = text.len();
        text.push_str(&span.text);
        let mut style = HighlightStyle::default();
        let mut styled = false;
        if span.bold {
            style.font_weight = Some(FontWeight::BOLD);
            styled = true;
        }
        if span.italic {
            style.font_style = Some(FontStyle::Italic);
            styled = true;
        }
        if span.strike {
            style.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            });
            styled = true;
        }
        if span.code {
            style.background_color = Some(theme::bg_hover());
            style.color = Some(theme::accent());
            styled = true;
        }
        if span.link {
            style.color = Some(theme::accent());
            style.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(theme::accent()),
                wavy: false,
            });
            styled = true;
        }
        if styled && !span.text.is_empty() {
            ranges.push((start..text.len(), style));
        }
    }
    (text, ranges)
}

fn styled_inline(inline: &Inline) -> StyledText {
    let (text, ranges) = inline_highlights(inline);
    StyledText::new(text).with_highlights(ranges)
}

/// One monospace code line as a single element, gutter optional.
fn code_line(line: &Line) -> AnyElement {
    let (text, ranges) = run_highlights(line);
    if text.is_empty() {
        // An empty line must still take a row's height.
        return div().h(px(16.)).into_any_element();
    }
    div()
        .whitespace_nowrap()
        .child(StyledText::new(text).with_highlights(ranges))
        .into_any_element()
}

pub(super) fn code_state(
    lines: &[Line],
    truncated: bool,
    handle: &gpui::ScrollHandle,
) -> AnyElement {
    let gutter = px(38.0);
    div()
        .id("preview-code")
        .track_scroll(handle)
        .flex()
        .flex_1()
        .min_h_0()
        .overflow_scroll()
        .px_2()
        .py_2()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .font_family(theme::MONO_FAMILY)
                .text_size(px(11.5))
                .line_height(px(16.0))
                .text_color(theme::text())
                .children(lines.iter().enumerate().map(|(index, line)| {
                    div()
                        .flex()
                        .flex_row()
                        .child(
                            div()
                                .w(gutter)
                                .flex_shrink_0()
                                .pr_2()
                                .flex()
                                .justify_end()
                                .text_color(theme::text_faint())
                                .child((index + 1).to_string()),
                        )
                        // 不能 min_w_0：长行要撑开内容宽度，外层才有东西可横向滚动
                        .child(div().flex_shrink_0().child(code_line(line)))
                }))
                .when(truncated, |this| {
                    this.child(
                        div()
                            .mt_2()
                            .text_xs()
                            .text_color(theme::text_faint())
                            .child("… truncated preview"),
                    )
                }),
        )
        .into_any_element()
}

// ---- markdown ----------------------------------------------------------------

pub(super) fn markdown_state(blocks: &[Block], truncated: bool) -> AnyElement {
    div()
        .id("preview-markdown")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px_5()
        .py_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .text_color(theme::text())
                .children(blocks.iter().map(render_block))
                .when(truncated, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme::text_faint())
                            .child("… truncated preview"),
                    )
                }),
        )
        .into_any_element()
}

fn heading_size(level: u8) -> f32 {
    match level {
        1 => 26.0,
        2 => 21.0,
        3 => 17.0,
        4 => 15.0,
        _ => 13.5,
    }
}

fn render_block(block: &Block) -> AnyElement {
    match block {
        Block::Heading { level, inline } => div()
            .mt_2()
            .pb_1()
            .when(*level <= 2, |this| {
                this.border_b_1().border_color(theme::border_soft())
            })
            .text_size(px(heading_size(*level)))
            .font_weight(FontWeight::BOLD)
            .child(styled_inline(inline))
            .into_any_element(),
        Block::Paragraph(inline) => div()
            .text_sm()
            .line_height(px(21.))
            .child(styled_inline(inline))
            .into_any_element(),
        Block::Code { lang, text } => render_code_block(lang.as_deref(), text),
        Block::Quote(blocks) => div()
            .flex()
            .flex_col()
            .gap_2()
            .pl_3()
            .border_l_2()
            .border_color(theme::accent_dim())
            .text_color(theme::text_muted())
            .children(blocks.iter().map(render_block))
            .into_any_element(),
        Block::List { start, items } => div()
            .flex()
            .flex_col()
            .gap_1()
            .children(
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| render_list_item(*start, index, item)),
            )
            .into_any_element(),
        Block::Rule => div()
            .my_2()
            .h(px(1.))
            .bg(theme::border())
            .into_any_element(),
        Block::Table { header, rows } => render_md_table(header, rows),
    }
}

fn render_code_block(lang: Option<&str>, text: &str) -> AnyElement {
    let lang = lang.unwrap_or("");
    let lines = code::highlight(text, "", lang, theme::is_dark(), 200);
    let body: Vec<AnyElement> = match lines {
        Some(lines) => lines.iter().map(code_line).collect(),
        None => text
            .lines()
            .take(200)
            .map(|line| {
                div()
                    .whitespace_nowrap()
                    .min_h(px(16.))
                    .child(line.replace('\t', "    "))
                    .into_any_element()
            })
            .collect(),
    };
    div()
        .flex()
        .flex_col()
        .p_3()
        .rounded_md()
        .bg(theme::bg_raised())
        .border_1()
        .border_color(theme::border_soft())
        .overflow_hidden()
        .font_family(theme::MONO_FAMILY)
        .text_size(px(11.5))
        .line_height(px(16.))
        .children(body)
        .into_any_element()
}

fn render_list_item(start: Option<u64>, index: usize, item: &ListItem) -> AnyElement {
    let marker = match (item.task, start) {
        (Some(true), _) => "☑".to_string(),
        (Some(false), _) => "☐".to_string(),
        (None, Some(first)) => format!("{}.", first + index as u64),
        (None, None) => "•".to_string(),
    };
    div()
        .flex()
        .flex_row()
        .gap_2()
        .text_sm()
        .child(
            div()
                .w(px(20.))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_color(theme::text_muted())
                .child(marker),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .children(item.blocks.iter().map(render_block)),
        )
        .into_any_element()
}

fn render_md_table(header: &[Inline], rows: &[Vec<Inline>]) -> AnyElement {
    let cell = |inline: &Inline, bold: bool| {
        div()
            .flex_1()
            .min_w_0()
            .px_2()
            .py_1()
            .text_xs()
            .when(bold, |this| this.font_weight(FontWeight::BOLD))
            .child(styled_inline(inline))
    };
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(theme::border_soft())
        .rounded_md()
        .overflow_hidden()
        .child(
            div()
                .flex()
                .flex_row()
                .bg(theme::bg_raised())
                .border_b_1()
                .border_color(theme::border_soft())
                .children(header.iter().map(|inline| cell(inline, true))),
        )
        .children(rows.iter().map(|row| {
            div()
                .flex()
                .flex_row()
                .border_b_1()
                .border_color(theme::border_soft())
                .children(row.iter().map(|inline| cell(inline, false)))
        }))
        .into_any_element()
}

// ---- csv / tsv ---------------------------------------------------------------

/// Pixel width per column: wide enough for the longest cell (capped), so a
/// column of IDs stays narrow and a column of titles gets room.
pub(super) fn column_widths(rows: &[Vec<String>]) -> Vec<f32> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    (0..columns)
        .map(|column| {
            let longest = rows
                .iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0);
            (longest.clamp(3, 28) as f32) * 7.2 + 20.0
        })
        .collect()
}

pub(super) fn table_state(rows: &[Vec<String>], truncated: bool) -> AnyElement {
    let widths = column_widths(rows);
    div()
        .id("preview-table")
        .flex_1()
        .min_h_0()
        .overflow_scroll()
        .p_2()
        .child(
            div()
                .flex()
                .flex_col()
                .text_xs()
                .children(rows.iter().enumerate().map(|(row_index, row)| {
                    div()
                        .flex()
                        .flex_row()
                        .when(row_index == 0, |this| {
                            this.bg(theme::bg_raised()).font_weight(FontWeight::BOLD)
                        })
                        .border_b_1()
                        .border_color(theme::border_soft())
                        .children(row.iter().enumerate().map(|(column, cell)| {
                            div()
                                .w(px(widths.get(column).copied().unwrap_or(60.0)))
                                .flex_shrink_0()
                                .px_2()
                                .py_1()
                                .truncate()
                                .child(cell.clone())
                        }))
                }))
                .when(truncated, |this| {
                    this.child(
                        div()
                            .mt_2()
                            .text_color(theme::text_faint())
                            .child("… truncated preview"),
                    )
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::{code::Run, markdown::Span};

    fn span(text: &str) -> Span {
        Span {
            text: text.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn code_runs_become_contiguous_ranges_over_the_joined_text() {
        let line: Line = vec![
            Run {
                text: "let".into(),
                rgb: 0xff0000,
                bold: true,
                italic: false,
            },
            Run {
                text: " x".into(),
                rgb: 0x00ff00,
                bold: false,
                italic: true,
            },
        ];
        let (text, ranges) = run_highlights(&line);
        assert_eq!(text, "let x");
        assert_eq!(ranges[0].0, 0..3);
        assert_eq!(ranges[1].0, 3..5);
        assert_eq!(ranges[0].1.font_weight, Some(FontWeight::BOLD));
        assert_eq!(ranges[1].1.font_style, Some(FontStyle::Italic));
    }

    #[test]
    fn only_styled_inline_spans_get_ranges() {
        let inline: Inline = vec![
            span("plain "),
            Span {
                text: "bold".into(),
                bold: true,
                ..Default::default()
            },
            span(" tail"),
        ];
        let (text, ranges) = inline_highlights(&inline);
        assert_eq!(text, "plain bold tail");
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].0, 6..10);
    }

    #[test]
    fn code_and_link_spans_are_tinted() {
        let inline: Inline = vec![
            Span {
                text: "c".into(),
                code: true,
                ..Default::default()
            },
            Span {
                text: "l".into(),
                link: true,
                ..Default::default()
            },
        ];
        let (_, ranges) = inline_highlights(&inline);
        assert!(ranges[0].1.background_color.is_some());
        assert!(ranges[1].1.underline.is_some());
    }

    #[test]
    fn multibyte_text_keeps_byte_ranges_on_char_boundaries() {
        let inline: Inline = vec![
            span("你好"),
            Span {
                text: "世界".into(),
                italic: true,
                ..Default::default()
            },
        ];
        let (text, ranges) = inline_highlights(&inline);
        assert!(text.is_char_boundary(ranges[0].0.start));
        assert_eq!(ranges[0].0, 6..12);
    }

    #[test]
    fn column_widths_follow_the_longest_cell_within_bounds() {
        let rows = vec![
            vec![
                "id".to_string(),
                "a very very long title indeed more than cap".to_string(),
            ],
            vec!["7".to_string(), "short".to_string()],
        ];
        let widths = column_widths(&rows);
        assert_eq!(widths.len(), 2);
        assert!(widths[0] < widths[1]);
        assert_eq!(widths[1], 28.0 * 7.2 + 20.0, "capped");
        assert!(column_widths(&[]).is_empty());
    }

    #[test]
    fn heading_sizes_shrink_with_depth() {
        assert!(heading_size(1) > heading_size(2));
        assert!(heading_size(2) > heading_size(3));
        assert_eq!(heading_size(5), heading_size(6));
    }
}
