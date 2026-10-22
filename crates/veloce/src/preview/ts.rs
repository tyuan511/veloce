//! Tree-sitter syntax highlighting, the engine Zed uses.
//!
//! Tree-sitter parses the whole file into a real syntax tree, so it colours
//! TypeScript types, TOML tables or Kotlin declarations that a regex grammar
//! mislabels. Languages without a grammar here fall back to syntect (see
//! `code::highlight`).

use std::sync::{Mutex, OnceLock};

use tree_sitter::Language;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use super::code::{Line, Run};

/// Capture names the highlight queries are matched against. A capture such as
/// `@keyword.function` binds to the longest listed prefix, so the list holds the
/// current names plus the older nvim-style ones some queries still use.
const NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "character",
    "comment",
    "conditional",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "escape",
    "exception",
    "field",
    "float",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
    "include",
    "keyword",
    "keyword.function",
    "keyword.operator",
    "keyword.return",
    "label",
    "method",
    "module",
    "namespace",
    "number",
    "operator",
    "parameter",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "repeat",
    "string",
    "string.escape",
    "string.regexp",
    "string.special",
    "tag",
    "tag.attribute",
    "type",
    "type.builtin",
    "type.definition",
    "variable",
    "variable.builtin",
    "variable.member",
    "variable.parameter",
    "text.title",
    "text.literal",
    "text.emphasis",
    "text.strong",
    "text.uri",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Comment,
    String,
    Number,
    Keyword,
    Function,
    Type,
    Property,
    Tag,
    Operator,
    Escape,
    Parameter,
    Heading,
    Plain,
}

/// Which colour group a capture name belongs to.
fn group_of(name: &str) -> Group {
    let head = name.split('.').next().unwrap_or(name);
    match name {
        "string.escape" | "escape" => return Group::Escape,
        "variable.parameter" | "parameter" => return Group::Parameter,
        "variable.builtin" | "constant.builtin" | "type.builtin" if head != "type" => {
            return Group::Number;
        }
        "tag.attribute" | "attribute" => return Group::Property,
        "text.title" => return Group::Heading,
        _ => {}
    }
    match head {
        "comment" => Group::Comment,
        "string" | "character" | "text" | "embedded" => Group::String,
        "number" | "float" | "boolean" | "constant" => Group::Number,
        "keyword" | "conditional" | "repeat" | "include" | "exception" | "label" => Group::Keyword,
        "function" | "method" | "constructor" => Group::Function,
        "type" | "module" | "namespace" => Group::Type,
        "property" | "field" => Group::Property,
        "variable" => {
            if name == "variable.member" {
                Group::Property
            } else {
                Group::Plain
            }
        }
        "tag" => Group::Tag,
        "operator" | "punctuation" => Group::Operator,
        _ => Group::Plain,
    }
}

/// `(0xRRGGBB, bold, italic)` for a group, tuned for a dark or a light surface.
fn style_of(group: Group, dark: bool) -> (u32, bool, bool) {
    let (dark_rgb, light_rgb, bold, italic) = match group {
        Group::Comment => (0x7F848E, 0xA0A1A7, false, true),
        Group::String => (0x98C379, 0x50A14F, false, false),
        Group::Number => (0xD19A66, 0x986801, false, false),
        Group::Keyword => (0xC678DD, 0xA626A4, false, false),
        Group::Function => (0x61AFEF, 0x4078F2, false, false),
        Group::Type => (0xE5C07B, 0xC18401, false, false),
        Group::Property => (0x56B6C2, 0x0184BC, false, false),
        Group::Tag => (0xE06C75, 0xE45649, false, false),
        Group::Operator => (0xABB2BF, 0x696C77, false, false),
        Group::Escape => (0x56B6C2, 0x0184BC, false, false),
        Group::Parameter => (0xE5A06E, 0xB76B01, false, false),
        Group::Heading => (0x61AFEF, 0x4078F2, true, false),
        Group::Plain => (0xD7DAE0, 0x383A42, false, false),
    };
    (if dark { dark_rgb } else { light_rgb }, bold, italic)
}

/// One supported language: how to recognise it and how to build its grammar.
struct Lang {
    name: &'static str,
    extensions: &'static [&'static str],
    file_names: &'static [&'static str],
    build: fn() -> Option<HighlightConfiguration>,
}

fn config(
    language: tree_sitter_language::LanguageFn,
    name: &str,
    highlights: &str,
    locals: &str,
) -> Option<HighlightConfiguration> {
    let mut config =
        HighlightConfiguration::new(Language::new(language), name, highlights, "", locals).ok()?;
    config.configure(NAMES);
    Some(config)
}

const LANGS: &[Lang] = &[
    Lang {
        name: "Rust",
        extensions: &["rs"],
        file_names: &[],
        build: || {
            config(
                tree_sitter_rust::LANGUAGE,
                "rust",
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "JavaScript",
        extensions: &["js", "mjs", "cjs", "jsx"],
        file_names: &[],
        build: || {
            config(
                tree_sitter_javascript::LANGUAGE,
                "javascript",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::LOCALS_QUERY,
            )
        },
    },
    Lang {
        name: "TypeScript",
        extensions: &["ts", "mts", "cts"],
        file_names: &[],
        // The TypeScript query only adds to JavaScript's, so both are needed.
        build: || {
            let query = format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            );
            config(
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
                "typescript",
                &query,
                tree_sitter_typescript::LOCALS_QUERY,
            )
        },
    },
    Lang {
        name: "TSX",
        extensions: &["tsx"],
        file_names: &[],
        build: || {
            let query = format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            );
            config(
                tree_sitter_typescript::LANGUAGE_TSX,
                "tsx",
                &query,
                tree_sitter_typescript::LOCALS_QUERY,
            )
        },
    },
    Lang {
        name: "Python",
        extensions: &["py", "pyi", "pyw"],
        file_names: &[],
        build: || {
            config(
                tree_sitter_python::LANGUAGE,
                "python",
                tree_sitter_python::HIGHLIGHTS_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "Go",
        extensions: &["go"],
        file_names: &[],
        build: || {
            config(
                tree_sitter_go::LANGUAGE,
                "go",
                tree_sitter_go::HIGHLIGHTS_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "JSON",
        extensions: &["json", "jsonc", "webmanifest", "ipynb"],
        file_names: &[],
        build: || {
            config(
                tree_sitter_json::LANGUAGE,
                "json",
                tree_sitter_json::HIGHLIGHTS_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "TOML",
        extensions: &["toml"],
        file_names: &["Cargo.lock", "Pipfile", "poetry.lock"],
        build: || {
            config(
                tree_sitter_toml_ng::LANGUAGE,
                "toml",
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "YAML",
        extensions: &["yaml", "yml"],
        file_names: &[],
        build: || {
            config(
                tree_sitter_yaml::LANGUAGE,
                "yaml",
                tree_sitter_yaml::HIGHLIGHTS_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "Kotlin",
        extensions: &["kt", "kts"],
        file_names: &[],
        // The grammar crate ships no query constant; ours is vendored from its
        // repository (see assets/queries).
        build: || {
            config(
                tree_sitter_kotlin_sg::LANGUAGE,
                "kotlin",
                include_str!("../../assets/queries/kotlin-highlights.scm"),
                "",
            )
        },
    },
    Lang {
        name: "Bash",
        extensions: &["sh", "bash", "zsh", "ksh"],
        file_names: &[".bashrc", ".zshrc", ".profile", ".bash_profile"],
        build: || {
            config(
                tree_sitter_bash::LANGUAGE,
                "bash",
                tree_sitter_bash::HIGHLIGHT_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "C",
        extensions: &["c", "h"],
        file_names: &[],
        build: || {
            config(
                tree_sitter_c::LANGUAGE,
                "c",
                tree_sitter_c::HIGHLIGHT_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "C++",
        extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx", "ino"],
        file_names: &[],
        build: || {
            let query = format!(
                "{}\n{}",
                tree_sitter_c::HIGHLIGHT_QUERY,
                tree_sitter_cpp::HIGHLIGHT_QUERY
            );
            config(tree_sitter_cpp::LANGUAGE, "cpp", &query, "")
        },
    },
    Lang {
        name: "Java",
        extensions: &["java"],
        file_names: &[],
        build: || {
            config(
                tree_sitter_java::LANGUAGE,
                "java",
                tree_sitter_java::HIGHLIGHTS_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "C#",
        extensions: &["cs"],
        file_names: &[],
        build: || {
            config(
                tree_sitter_c_sharp::LANGUAGE,
                "c_sharp",
                tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "HTML",
        extensions: &["html", "htm", "xhtml"],
        file_names: &[],
        build: || {
            config(
                tree_sitter_html::LANGUAGE,
                "html",
                tree_sitter_html::HIGHLIGHTS_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "CSS",
        extensions: &["css"],
        file_names: &[],
        build: || {
            config(
                tree_sitter_css::LANGUAGE,
                "css",
                tree_sitter_css::HIGHLIGHTS_QUERY,
                "",
            )
        },
    },
    Lang {
        name: "Ruby",
        extensions: &["rb", "rake", "gemspec", "ru"],
        file_names: &["Rakefile", "Gemfile", "Podfile", "Fastfile"],
        build: || {
            config(
                tree_sitter_ruby::LANGUAGE,
                "ruby",
                tree_sitter_ruby::HIGHLIGHTS_QUERY,
                tree_sitter_ruby::LOCALS_QUERY,
            )
        },
    },
    Lang {
        name: "Lua",
        extensions: &["lua"],
        file_names: &[],
        build: || {
            config(
                tree_sitter_lua::LANGUAGE,
                "lua",
                tree_sitter_lua::HIGHLIGHTS_QUERY,
                tree_sitter_lua::LOCALS_QUERY,
            )
        },
    },
];

/// The grammar entry for a file, by exact name first and then by extension.
fn find_lang(file_name: &str, ext: &str) -> Option<usize> {
    LANGS
        .iter()
        .position(|lang| lang.file_names.contains(&file_name))
        .or_else(|| {
            let ext = ext.to_ascii_lowercase();
            LANGS
                .iter()
                .position(|lang| lang.extensions.contains(&ext.as_str()))
        })
}

/// Display name of the grammar that would highlight this file.
pub fn language_name(file_name: &str, ext: &str) -> Option<&'static str> {
    find_lang(file_name, ext).map(|index| LANGS[index].name)
}

/// Grammars are costly to build (their queries compile on first use), so each is
/// built once and kept.
fn configuration(index: usize) -> Option<&'static HighlightConfiguration> {
    static CACHE: OnceLock<Vec<OnceLock<Option<HighlightConfiguration>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| LANGS.iter().map(|_| OnceLock::new()).collect());
    cache[index].get_or_init(LANGS[index].build).as_ref()
}

/// The first `max_lines` lines of `text` as one slice.
fn head_lines(text: &str, max_lines: usize) -> &str {
    let mut end = 0;
    for (count, line) in text.split_inclusive('\n').enumerate() {
        if count >= max_lines {
            break;
        }
        end += line.len();
    }
    &text[..end]
}

/// Highlight up to `max_lines` lines, or `None` if there is no grammar for the
/// file or highlighting failed (the caller then falls back).
pub fn highlight(
    text: &str,
    file_name: &str,
    ext: &str,
    dark: bool,
    max_lines: usize,
) -> Option<Vec<Line>> {
    let index = find_lang(file_name, ext)?;
    let config = configuration(index)?;
    let source = head_lines(text, max_lines);
    if source.is_empty() {
        return Some(Vec::new());
    }

    // The highlighter keeps parser state between calls, so reuse is worthwhile
    // but it is not `Sync`; one lock keeps previews simple.
    static HIGHLIGHTER: OnceLock<Mutex<Highlighter>> = OnceLock::new();
    let mut highlighter = HIGHLIGHTER
        .get_or_init(|| Mutex::new(Highlighter::new()))
        .lock()
        .ok()?;
    let events = highlighter
        .highlight(config, source.as_bytes(), None, None, |_| None)
        .ok()?;

    let plain = style_of(Group::Plain, dark);
    let mut stack: Vec<usize> = Vec::new();
    let mut lines: Vec<Line> = vec![Vec::new()];

    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(highlight) => stack.push(highlight.0),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let (rgb, bold, italic) = match stack.last() {
                    Some(&index) => style_of(group_of(NAMES[index]), dark),
                    None => plain,
                };
                let piece = &source[start..end];
                let mut parts = piece.split('\n').peekable();
                while let Some(part) = parts.next() {
                    push_run(lines.last_mut()?, part, rgb, bold, italic);
                    if parts.peek().is_some() {
                        lines.push(Vec::new());
                    }
                }
            }
        }
    }
    // A trailing newline opens an empty last line that is not really there.
    if lines.last().is_some_and(Vec::is_empty) && source.ends_with('\n') {
        lines.pop();
    }
    Some(lines)
}

fn push_run(line: &mut Line, text: &str, rgb: u32, bold: bool, italic: bool) {
    let text = text.trim_end_matches('\r').replace('\t', "    ");
    if text.is_empty() {
        return;
    }
    // Neighbours with the same colour merge, keeping runs (and elements) few.
    if let Some(last) = line.last_mut()
        && last.rgb == rgb
        && last.bold == bold
        && last.italic == italic
    {
        last.text.push_str(&text);
        return;
    }
    line.push(Run {
        text,
        rgb,
        bold,
        italic,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn join(line: &Line) -> String {
        line.iter().map(|run| run.text.as_str()).collect()
    }

    fn colours(lines: &[Line]) -> HashSet<u32> {
        lines.iter().flatten().map(|run| run.rgb).collect()
    }

    /// A small program per language; every grammar must produce more than one
    /// colour, which catches a query that silently failed to bind.
    const SAMPLES: &[(&str, &str, &str)] = &[
        (
            "a.rs",
            "rs",
            "fn main() {\n    let x: u32 = 42; // note\n    println!(\"{x}\");\n}\n",
        ),
        (
            "a.js",
            "js",
            "const x = 1;\nfunction f(a) { return a + \"s\"; } // c\n",
        ),
        (
            "a.jsx",
            "jsx",
            "const el = <div className=\"a\">hi</div>;\n",
        ),
        (
            "a.ts",
            "ts",
            "interface A { n: number }\nconst x: A = { n: 1 };\nfunction f(a: string): void {}\n",
        ),
        (
            "a.tsx",
            "tsx",
            "const C = (p: { n: number }) => <b>{p.n}</b>;\n",
        ),
        (
            "a.py",
            "py",
            "def f(x):\n    return x + 1  # c\n\nclass A: pass\n",
        ),
        (
            "a.go",
            "go",
            "package main\nfunc main() {\n\tx := 1\n\t_ = x\n}\n",
        ),
        ("a.json", "json", "{\"a\": [1, true, null], \"b\": \"s\"}\n"),
        (
            "a.toml",
            "toml",
            "[package]\nname = \"veloce\"\nversion = \"0.1.0\"\n# c\nn = 3\n",
        ),
        ("a.yaml", "yaml", "name: veloce\nlist:\n  - a\n  - 1\n# c\n"),
        (
            "a.kt",
            "kt",
            "fun main() {\n    val x: Int = 1\n    println(\"hi $x\")\n}\nclass A\n",
        ),
        (
            "a.sh",
            "sh",
            "#!/bin/bash\nif [ -f a ]; then echo \"x\"; fi\nFOO=1\n",
        ),
        (
            "a.c",
            "c",
            "#include <stdio.h>\nint main(void) { return 0; }\n",
        ),
        (
            "a.cpp",
            "cpp",
            "#include <vector>\nclass A { public: int f() const; };\nstd::vector<int> v;\n",
        ),
        (
            "a.java",
            "java",
            "public class A {\n  private int x = 1;\n  void f() {}\n}\n",
        ),
        (
            "a.cs",
            "cs",
            "class A { public int X { get; set; } void F() { var y = 1; } }\n",
        ),
        (
            "a.html",
            "html",
            "<!doctype html>\n<div class=\"a\">hi</div>\n",
        ),
        (
            "a.css",
            "css",
            ".a { color: #fff; margin: 0 }\n@media screen { .b {} }\n",
        ),
        ("a.rb", "rb", "class A\n  def f(x)\n    x + 1\n  end\nend\n"),
        (
            "a.lua",
            "lua",
            "local function f(x)\n  return x + 1\nend\n-- c\n",
        ),
    ];

    #[test]
    fn every_grammar_loads_and_colours_its_sample() {
        for (name, ext, code) in SAMPLES {
            let lines = highlight(code, name, ext, true, 100)
                .unwrap_or_else(|| panic!("{name}: grammar failed to load"));
            assert!(
                colours(&lines).len() > 1,
                "{name}: expected several colours, got {:?}",
                colours(&lines)
            );
        }
    }

    #[test]
    fn highlighting_never_changes_the_text() {
        for (name, ext, code) in SAMPLES {
            if code.contains('\t') {
                continue; // tabs are expanded on purpose
            }
            let lines = highlight(code, name, ext, true, 100).unwrap();
            let rebuilt: Vec<String> = lines.iter().map(join).collect();
            let original: Vec<&str> = code.lines().collect();
            assert_eq!(rebuilt, original, "{name}");
        }
    }

    #[test]
    fn typescript_types_get_their_own_colour_unlike_plain_identifiers() {
        let lines = highlight("const x: number = y;\n", "a.ts", "ts", true, 10).unwrap();
        let colour_of = |text: &str| {
            lines[0]
                .iter()
                .find(|run| run.text.contains(text))
                .map(|run| run.rgb)
        };
        assert_ne!(colour_of("number"), colour_of("y"));
    }

    #[test]
    fn toml_tables_keys_and_strings_differ() {
        let lines = highlight("[package]\nname = \"v\"\n", "Cargo.toml", "toml", true, 10).unwrap();
        assert!(colours(&lines).len() >= 3);
    }

    #[test]
    fn kotlin_keywords_are_coloured() {
        let lines = highlight("fun main() {}\n", "a.kt", "kt", true, 10).unwrap();
        let fun = lines[0].iter().find(|r| r.text.contains("fun")).unwrap();
        assert_ne!(fun.rgb, style_of(Group::Plain, true).0);
    }

    #[test]
    fn unknown_extensions_have_no_grammar() {
        assert!(highlight("x", "a.zzz", "zzz", true, 10).is_none());
        assert_eq!(language_name("a.zzz", "zzz"), None);
    }

    #[test]
    fn file_names_and_extension_case_both_select_a_language() {
        assert_eq!(language_name("Rakefile", ""), Some("Ruby"));
        assert_eq!(language_name("Cargo.lock", "lock"), Some("TOML"));
        assert_eq!(language_name("MAIN.RS", "RS"), Some("Rust"));
        assert_eq!(language_name("a.tsx", "tsx"), Some("TSX"));
        assert_eq!(language_name("a.ts", "ts"), Some("TypeScript"));
    }

    #[test]
    fn the_line_cap_limits_the_work_and_the_result() {
        let code = "let a = 1;\n".repeat(500);
        let lines = highlight(&code, "a.js", "js", true, 7).unwrap();
        assert_eq!(lines.len(), 7);
    }

    #[test]
    fn blank_lines_survive_and_a_final_newline_adds_no_phantom_line() {
        let lines = highlight("let a = 1;\n\nlet b = 2;\n", "a.js", "js", true, 10).unwrap();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].is_empty());
        let no_newline = highlight("let a = 1;", "a.js", "js", true, 10).unwrap();
        assert_eq!(no_newline.len(), 1);
    }

    #[test]
    fn tabs_become_four_spaces_and_crlf_loses_its_carriage_return() {
        let lines = highlight("if (a) {\r\n\tb();\r\n}\r\n", "a.js", "js", true, 10).unwrap();
        assert_eq!(join(&lines[1]), "    b();");
        assert!(!join(&lines[0]).contains('\r'));
    }

    #[test]
    fn broken_syntax_still_highlights_without_failing() {
        assert!(highlight("fn (((", "a.rs", "rs", true, 10).is_some());
    }

    #[test]
    fn empty_input_yields_no_lines() {
        assert_eq!(highlight("", "a.rs", "rs", true, 10).unwrap().len(), 0);
    }

    #[test]
    fn dark_and_light_use_different_palettes() {
        let dark = highlight("let a = 1;\n", "a.js", "js", true, 5).unwrap();
        let light = highlight("let a = 1;\n", "a.js", "js", false, 5).unwrap();
        assert_ne!(colours(&dark), colours(&light));
    }

    #[test]
    fn comments_are_italic_and_headings_bold() {
        let lines = highlight("// note\n", "a.js", "js", true, 5).unwrap();
        assert!(lines[0][0].italic);
        assert!(style_of(Group::Heading, true).1);
    }

    #[test]
    fn capture_names_map_to_sensible_groups() {
        assert_eq!(group_of("comment"), Group::Comment);
        assert_eq!(group_of("keyword.return"), Group::Keyword);
        assert_eq!(group_of("conditional"), Group::Keyword);
        assert_eq!(group_of("string.escape"), Group::Escape);
        assert_eq!(group_of("function.builtin"), Group::Function);
        assert_eq!(group_of("type.builtin"), Group::Type);
        assert_eq!(group_of("variable.parameter"), Group::Parameter);
        assert_eq!(group_of("punctuation.bracket"), Group::Operator);
        assert_eq!(group_of("variable"), Group::Plain);
        assert_eq!(group_of("something.unknown"), Group::Plain);
    }

    #[test]
    fn head_lines_keeps_whole_lines_only() {
        assert_eq!(head_lines("a\nb\nc\n", 2), "a\nb\n");
        assert_eq!(head_lines("a\nb", 5), "a\nb");
        assert_eq!(head_lines("a\nb\n", 0), "");
        assert_eq!(head_lines("", 3), "");
    }
}
