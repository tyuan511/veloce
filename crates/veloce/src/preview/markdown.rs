//! Markdown → a small block tree the preview can draw natively.
//!
//! pulldown-cmark does the parsing; this module only folds its event stream into
//! blocks with styled inline spans, which keeps rendering trivial and the
//! structure testable. Raw HTML is dropped: it is usually badges and layout hints
//! that would show up as noise.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
}

pub type Inline = Vec<Span>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// `Some(done)` for a `- [ ]` / `- [x]` task.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        inline: Inline,
    },
    Paragraph(Inline),
    Code {
        lang: Option<String>,
        text: String,
    },
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Rule,
    Table {
        header: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
}

type Events<'a> = std::iter::Peekable<Parser<'a>>;

pub fn parse(source: &str) -> Vec<Block> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut events = Parser::new_ext(source, options).peekable();
    parse_blocks(&mut events, None)
}

fn level_number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Collect blocks until the end tag `end` (or the end of input).
fn parse_blocks(events: &mut Events, end: Option<TagEnd>) -> Vec<Block> {
    let mut blocks = Vec::new();
    // Tight list items put their text straight inside the item, with no paragraph.
    let mut loose: Inline = Vec::new();
    let flush = |loose: &mut Inline, blocks: &mut Vec<Block>| {
        if !loose.is_empty() {
            blocks.push(Block::Paragraph(std::mem::take(loose)));
        }
    };

    while let Some(event) = events.next() {
        match event {
            Event::End(tag) if Some(tag) == end => break,
            Event::Start(Tag::Heading { level, .. }) => {
                flush(&mut loose, &mut blocks);
                let inline = parse_inline(events, TagEnd::Heading(level));
                blocks.push(Block::Heading {
                    level: level_number(level),
                    inline,
                });
            }
            Event::Start(Tag::Paragraph) => {
                flush(&mut loose, &mut blocks);
                let inline = parse_inline(events, TagEnd::Paragraph);
                blocks.push(Block::Paragraph(inline));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut loose, &mut blocks);
                let lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .filter(|word| !word.is_empty())
                        .map(str::to_string),
                    pulldown_cmark::CodeBlockKind::Indented => None,
                };
                let mut text = String::new();
                for inner in events.by_ref() {
                    match inner {
                        Event::Text(chunk) => text.push_str(&chunk),
                        Event::End(TagEnd::CodeBlock) => break,
                        _ => {}
                    }
                }
                blocks.push(Block::Code {
                    lang,
                    text: text.trim_end_matches('\n').to_string(),
                });
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut loose, &mut blocks);
                blocks.push(Block::Quote(parse_blocks(
                    events,
                    Some(TagEnd::BlockQuote(None)),
                )));
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut loose, &mut blocks);
                blocks.push(parse_list(events, start));
            }
            Event::Start(Tag::Table(_)) => {
                flush(&mut loose, &mut blocks);
                blocks.push(parse_table(events));
            }
            Event::Rule => {
                flush(&mut loose, &mut blocks);
                blocks.push(Block::Rule);
            }
            // Inline content directly inside a tight list item.
            other => {
                if let Some(span) = inline_event(&other, &mut Style::default()) {
                    push_span(&mut loose, span);
                } else if let Event::Start(tag) = &other
                    && is_inline_tag(tag)
                {
                    let (inline, _) = parse_inline_from(events, tag.clone());
                    for span in inline {
                        push_span(&mut loose, span);
                    }
                }
            }
        }
    }
    flush(&mut loose, &mut blocks);
    blocks
}

fn parse_list(events: &mut Events, start: Option<u64>) -> Block {
    let mut items = Vec::new();
    while let Some(event) = events.next() {
        match event {
            Event::End(TagEnd::List(_)) => break,
            Event::Start(Tag::Item) => {
                let task = match events.peek() {
                    Some(Event::TaskListMarker(done)) => {
                        let done = *done;
                        events.next();
                        Some(done)
                    }
                    _ => None,
                };
                let blocks = parse_blocks(events, Some(TagEnd::Item));
                items.push(ListItem { task, blocks });
            }
            _ => {}
        }
    }
    Block::List { start, items }
}

fn parse_table(events: &mut Events) -> Block {
    let mut header: Vec<Inline> = Vec::new();
    let mut rows: Vec<Vec<Inline>> = Vec::new();
    let mut current: Vec<Inline> = Vec::new();
    while let Some(event) = events.next() {
        match event {
            Event::End(TagEnd::Table) => break,
            Event::Start(Tag::TableCell) => {
                current.push(parse_inline(events, TagEnd::TableCell));
            }
            Event::End(TagEnd::TableHead) => header = std::mem::take(&mut current),
            Event::End(TagEnd::TableRow) => rows.push(std::mem::take(&mut current)),
            _ => {}
        }
    }
    Block::Table { header, rows }
}

#[derive(Default, Clone, Copy)]
struct Style {
    bold: u32,
    italic: u32,
    strike: u32,
    link: u32,
}

fn is_inline_tag(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }
    )
}

fn push_span(inline: &mut Inline, span: Span) {
    // Merge neighbours with the same style so ranges stay few.
    if let Some(last) = inline.last_mut()
        && last.bold == span.bold
        && last.italic == span.italic
        && last.strike == span.strike
        && last.code == span.code
        && last.link == span.link
    {
        last.text.push_str(&span.text);
        return;
    }
    inline.push(span);
}

fn span(text: &str, style: &Style, code: bool) -> Span {
    Span {
        text: text.to_string(),
        bold: style.bold > 0,
        italic: style.italic > 0,
        strike: style.strike > 0,
        code,
        link: style.link > 0,
    }
}

/// Text-like events become spans; everything else is `None`.
fn inline_event(event: &Event, style: &mut Style) -> Option<Span> {
    match event {
        Event::Text(text) => Some(span(text, style, false)),
        Event::Code(text) => Some(span(text, style, true)),
        Event::SoftBreak => Some(span(" ", style, false)),
        Event::HardBreak => Some(span("\n", style, false)),
        Event::FootnoteReference(name) => Some(span(&format!("[{name}]"), style, false)),
        _ => None,
    }
}

fn apply(tag: &Tag, style: &mut Style, delta: i32) {
    let bump = |value: &mut u32| {
        *value = if delta > 0 {
            *value + 1
        } else {
            value.saturating_sub(1)
        };
    };
    match tag {
        Tag::Emphasis => bump(&mut style.italic),
        Tag::Strong => bump(&mut style.bold),
        Tag::Strikethrough => bump(&mut style.strike),
        Tag::Link { .. } => bump(&mut style.link),
        // Images show their alt text, italicised.
        Tag::Image { .. } => bump(&mut style.italic),
        _ => {}
    }
}

fn apply_end(end: &TagEnd, style: &mut Style) {
    match end {
        TagEnd::Emphasis | TagEnd::Image => style.italic = style.italic.saturating_sub(1),
        TagEnd::Strong => style.bold = style.bold.saturating_sub(1),
        TagEnd::Strikethrough => style.strike = style.strike.saturating_sub(1),
        TagEnd::Link => style.link = style.link.saturating_sub(1),
        _ => {}
    }
}

/// Inline content up to `end`.
fn parse_inline(events: &mut Events, end: TagEnd) -> Inline {
    let mut inline = Inline::new();
    let mut style = Style::default();
    for event in events.by_ref() {
        if matches!(&event, Event::End(tag) if *tag == end) {
            break;
        }
        match &event {
            Event::Start(tag) => apply(tag, &mut style, 1),
            Event::End(tag) => apply_end(tag, &mut style),
            other => {
                if let Some(piece) = inline_event(other, &mut style) {
                    push_span(&mut inline, piece);
                }
            }
        }
    }
    inline
}

/// Inline content of an emphasis/link/image that started at block level (inside a
/// tight list item). Returns the spans; the end tag is consumed.
fn parse_inline_from(events: &mut Events, start: Tag) -> (Inline, ()) {
    let mut inline = Inline::new();
    let mut style = Style::default();
    apply(&start, &mut style, 1);
    let mut depth = 1;
    for event in events.by_ref() {
        match &event {
            Event::Start(tag) => {
                apply(tag, &mut style, 1);
                depth += 1;
            }
            Event::End(tag) => {
                apply_end(tag, &mut style);
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            other => {
                if let Some(piece) = inline_event(other, &mut style) {
                    push_span(&mut inline, piece);
                }
            }
        }
    }
    (inline, ())
}

/// Plain text of an inline run, for tests.
#[cfg(test)]
pub fn plain(inline: &Inline) -> String {
    inline.iter().map(|span| span.text.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(blocks: &[Block]) -> &Block {
        blocks.first().expect("at least one block")
    }

    #[test]
    fn headings_carry_their_level_and_text() {
        let blocks = parse("# Title\n\n### Third\n");
        match (&blocks[0], &blocks[1]) {
            (
                Block::Heading {
                    level: 1,
                    inline: a,
                },
                Block::Heading {
                    level: 3,
                    inline: b,
                },
            ) => {
                assert_eq!(plain(a), "Title");
                assert_eq!(plain(b), "Third");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn inline_styles_split_a_paragraph_into_spans() {
        let blocks = parse("plain **bold** *it* `code` ~~gone~~ [link](http://x)\n");
        let Block::Paragraph(inline) = first(&blocks) else {
            panic!("paragraph expected");
        };
        let find = |text: &str| inline.iter().find(|s| s.text == text).unwrap();
        assert!(find("bold").bold);
        assert!(find("it").italic);
        assert!(find("code").code);
        assert!(find("gone").strike);
        assert!(find("link").link);
        assert!(!find("plain ").bold);
        assert_eq!(plain(inline), "plain bold it code gone link");
    }

    #[test]
    fn nested_emphasis_combines_styles() {
        let blocks = parse("***both***\n");
        let Block::Paragraph(inline) = first(&blocks) else {
            panic!()
        };
        assert!(inline[0].bold && inline[0].italic);
    }

    #[test]
    fn soft_breaks_become_spaces_and_hard_breaks_newlines() {
        let blocks = parse("a\nb  \nc\n");
        let Block::Paragraph(inline) = first(&blocks) else {
            panic!()
        };
        assert_eq!(plain(inline), "a b\nc");
    }

    #[test]
    fn fenced_code_keeps_language_and_text() {
        let blocks = parse("```rust\nfn main() {}\n```\n");
        assert_eq!(
            blocks[0],
            Block::Code {
                lang: Some("rust".into()),
                text: "fn main() {}".into()
            }
        );
        let plain_fence = parse("```\nx\n```\n");
        assert!(matches!(&plain_fence[0], Block::Code { lang: None, .. }));
    }

    #[test]
    fn unordered_and_ordered_lists_keep_their_items() {
        let blocks = parse("- a\n- b\n\n3. x\n4. y\n");
        match (&blocks[0], &blocks[1]) {
            (
                Block::List {
                    start: None,
                    items: bullets,
                },
                Block::List {
                    start: Some(3),
                    items: numbers,
                },
            ) => {
                assert_eq!(bullets.len(), 2);
                assert_eq!(numbers.len(), 2);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn tight_list_items_still_hold_a_paragraph_of_text() {
        let blocks = parse("- one\n- two\n");
        let Block::List { items, .. } = first(&blocks) else {
            panic!()
        };
        let Block::Paragraph(inline) = &items[0].blocks[0] else {
            panic!("tight item should wrap its text")
        };
        assert_eq!(plain(inline), "one");
    }

    #[test]
    fn nested_lists_nest() {
        let blocks = parse("- parent\n  - child\n");
        let Block::List { items, .. } = first(&blocks) else {
            panic!()
        };
        assert!(
            items[0]
                .blocks
                .iter()
                .any(|b| matches!(b, Block::List { .. }))
        );
    }

    #[test]
    fn task_list_markers_record_their_state() {
        let blocks = parse("- [x] done\n- [ ] todo\n- plain\n");
        let Block::List { items, .. } = first(&blocks) else {
            panic!()
        };
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[1].task, Some(false));
        assert_eq!(items[2].task, None);
    }

    #[test]
    fn block_quotes_hold_blocks() {
        let blocks = parse("> quoted\n> more\n");
        let Block::Quote(inner) = first(&blocks) else {
            panic!()
        };
        assert!(matches!(inner[0], Block::Paragraph(_)));
    }

    #[test]
    fn tables_split_into_header_and_rows() {
        let blocks = parse("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n");
        let Block::Table { header, rows } = first(&blocks) else {
            panic!("table expected")
        };
        assert_eq!(header.len(), 2);
        assert_eq!(plain(&header[1]), "b");
        assert_eq!(rows.len(), 2);
        assert_eq!(plain(&rows[1][0]), "3");
    }

    #[test]
    fn rules_and_raw_html() {
        let blocks = parse("---\n\n<p align=\"center\">x</p>\n\ntext\n");
        assert!(matches!(blocks[0], Block::Rule));
        assert!(
            blocks
                .iter()
                .all(|b| !matches!(b, Block::Paragraph(i) if plain(i).contains("<p"))),
            "html is dropped"
        );
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, Block::Paragraph(i) if plain(i) == "text"))
        );
    }

    #[test]
    fn empty_and_whitespace_input_have_no_blocks() {
        assert!(parse("").is_empty());
        assert!(parse("\n\n  \n").is_empty());
    }

    #[test]
    fn image_alt_text_is_shown_in_italics() {
        let blocks = parse("![logo](a.png)\n");
        let Block::Paragraph(inline) = first(&blocks) else {
            panic!()
        };
        assert_eq!(plain(inline), "logo");
        assert!(inline[0].italic);
    }
}
