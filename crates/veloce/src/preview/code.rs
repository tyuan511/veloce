//! Syntax highlighting for the text preview, using syntect's bundled grammars.
//!
//! Only foreground colours are taken from the theme; the preview paints its own
//! background so highlighted code matches the rest of the UI in light and dark.

use std::sync::OnceLock;

use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
};

/// One coloured piece of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    /// `0xRRGGBB`.
    pub rgb: u32,
    pub bold: bool,
    pub italic: bool,
}

pub type Line = Vec<Run>;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn themes() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(ThemeSet::load_defaults)
}

/// Extensions syntect's default pack lacks, mapped to a close relative so they
/// still get *some* colour.
fn canonical_extension(ext: &str) -> &str {
    match ext {
        "ts" | "tsx" | "jsx" | "mjs" | "cjs" | "mts" | "cts" => "js",
        "zsh" | "bash" | "fish" | "ksh" => "sh",
        "hpp" | "cc" | "cxx" | "hh" | "hxx" | "ino" => "cpp",
        "kt" | "kts" | "scala" => "java",
        "vue" | "svelte" | "htm" | "xhtml" => "html",
        "jsonc" | "json5" | "ipynb" | "webmanifest" => "json",
        "plist" | "svg" | "xsd" | "xsl" | "rss" | "atom" | "csproj" => "xml",
        "yml" => "yaml",
        "toml" | "conf" | "cfg" | "env" | "properties" => "ini",
        "mdx" => "md",
        other => other,
    }
}

/// The grammar for a file, by (aliased) extension, then by exact file name
/// (`Makefile`, `Dockerfile`), then by name given directly (fenced code blocks).
fn find_syntax(file_name: &str, ext: &str) -> Option<&'static SyntaxReference> {
    let set = syntaxes();
    let ext = canonical_extension(ext);
    set.find_syntax_by_extension(ext)
        .or_else(|| set.find_syntax_by_extension(file_name))
        .or_else(|| set.find_syntax_by_token(ext))
        .filter(|syntax| syntax.name != "Plain Text")
}

/// Display name of the language a file would be highlighted as.
#[cfg(test)]
fn syntect_language_name(file_name: &str, ext: &str) -> Option<String> {
    find_syntax(file_name, ext).map(|syntax| syntax.name.clone())
}

/// Display name of the language a file is highlighted as: tree-sitter's grammar
/// when there is one, otherwise syntect's.
#[cfg(test)]
pub fn language_name(file_name: &str, ext: &str) -> Option<String> {
    super::ts::language_name(file_name, ext)
        .map(str::to_string)
        .or_else(|| syntect_language_name(file_name, ext))
}

/// Highlight up to `max_lines` lines. Tree-sitter goes first (a real syntax tree);
/// syntect covers the languages it has no grammar for. `None` means neither knows
/// the language: the caller shows plain text rather than a pretend-highlighted wall
/// of one colour.
pub fn highlight(
    text: &str,
    file_name: &str,
    ext: &str,
    dark: bool,
    max_lines: usize,
) -> Option<Vec<Line>> {
    super::ts::highlight(text, file_name, ext, dark, max_lines)
        .or_else(|| highlight_syntect(text, file_name, ext, dark, max_lines))
}

fn highlight_syntect(
    text: &str,
    file_name: &str,
    ext: &str,
    dark: bool,
    max_lines: usize,
) -> Option<Vec<Line>> {
    let syntax = find_syntax(file_name, ext)?;
    let theme_name = if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    let theme = themes().themes.get(theme_name)?;
    let mut highlighter = HighlightLines::new(syntax, theme);

    let mut lines = Vec::new();
    for raw in text.lines().take(max_lines) {
        // syntect's newline-aware grammars want the terminator back.
        let with_newline = format!("{raw}\n");
        let regions = highlighter.highlight_line(&with_newline, syntaxes()).ok()?;
        let mut runs: Line = Vec::new();
        for (style, piece) in regions {
            let piece = piece.trim_end_matches('\n');
            if piece.is_empty() {
                continue;
            }
            let color = style.foreground;
            runs.push(Run {
                text: piece.replace('\t', "    "),
                rgb: (u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b),
                bold: style.font_style.contains(FontStyle::BOLD),
                italic: style.font_style.contains(FontStyle::ITALIC),
            });
        }
        lines.push(runs);
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn join(line: &Line) -> String {
        line.iter().map(|run| run.text.as_str()).collect()
    }

    #[test]
    fn rust_source_gets_several_colours_and_keeps_its_text() {
        let lines =
            highlight("fn main() {\n    let x = 1;\n}", "main.rs", "rs", true, 100).unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(join(&lines[0]), "fn main() {");
        assert_eq!(join(&lines[1]), "    let x = 1;", "indent preserved");
        let colours: std::collections::HashSet<u32> = lines[0].iter().map(|r| r.rgb).collect();
        assert!(colours.len() > 1, "keywords and punctuation differ");
    }

    #[test]
    fn unknown_languages_are_left_for_plain_text() {
        assert!(highlight("hello", "notes.zzzzz", "zzzzz", true, 10).is_none());
        assert!(highlight("hello", "notes.txt", "txt", true, 10).is_none());
    }

    #[test]
    fn typescript_uses_its_own_tree_sitter_grammar() {
        assert_eq!(language_name("a.ts", "ts").as_deref(), Some("TypeScript"));
        assert_eq!(language_name("a.tsx", "tsx").as_deref(), Some("TSX"));
        assert!(highlight("const a: number = 1;", "a.ts", "ts", true, 10).is_some());
    }

    #[test]
    fn languages_without_a_tree_sitter_grammar_fall_back_to_syntect() {
        assert!(super::super::ts::language_name("q.sql", "sql").is_none());
        assert!(highlight("SELECT 1;", "q.sql", "sql", true, 10).is_some());
        assert!(language_name("q.sql", "sql").is_some());
    }

    #[test]
    fn well_known_file_names_are_recognised() {
        assert!(highlight("all:\n\techo hi\n", "Makefile", "", true, 10).is_some());
    }

    #[test]
    fn the_line_cap_is_respected() {
        let text = "let a = 1;\n".repeat(50);
        assert_eq!(highlight(&text, "a.js", "js", true, 7).unwrap().len(), 7);
    }

    #[test]
    fn light_and_dark_themes_pick_different_colours() {
        let dark = highlight("fn main() {}", "a.rs", "rs", true, 1).unwrap();
        let light = highlight("fn main() {}", "a.rs", "rs", false, 1).unwrap();
        assert_ne!(dark[0], light[0]);
    }

    #[test]
    fn empty_lines_stay_in_the_output() {
        let lines = highlight("a = 1\n\nb = 2\n", "a.py", "py", true, 10).unwrap();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].is_empty());
    }

    #[test]
    fn json_and_yaml_have_grammars() {
        assert_eq!(language_name("a.json", "json").as_deref(), Some("JSON"));
        assert!(language_name("a.yaml", "yaml").is_some());
        assert!(language_name("a.yml", "yml").is_some());
    }

    #[test]
    fn language_names_are_absent_for_plain_files() {
        assert_eq!(language_name("a.log", "log"), None);
    }
}
