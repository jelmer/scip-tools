//! Generate a SCIP index of syntax-highlighting tokens. See the README for the
//! grammar set and usage.
//!
//! This relies on the codegraph web server treating `Occurrence.syntax_kind` as
//! the source of truth for highlighting and keeping symbol-less occurrences that
//! carry a highlightable kind: each occurrence we emit has a `syntax_kind` and
//! an empty `symbol`, so it highlights but offers no navigation. Positions are
//! UTF-16 code-unit offsets, the encoding the renderer assumes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser as ClapParser;
use scip::types::{
    Document, Index, Metadata, Occurrence, ProtocolVersion, SyntaxKind, TextEncoding, ToolInfo,
};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use SyntaxKind as K;

/// A capture-name to SCIP-kind mapping for one grammar. Names are matched
/// most-specific first (the longest matching entry wins), so an entry for
/// `string.special` takes precedence over a bare `string`. Capture names a
/// grammar's query uses but the map omits render unhighlighted.
type CaptureMap = &'static [(&'static str, SyntaxKind)];

/// The conventional tree-sitter highlight captures, shared by code grammars
/// (TOML, JSON, CSS, Lua, ...). Most grammars use this map unchanged.
const BASE: CaptureMap = &[
    ("comment", K::Comment),
    ("string", K::StringLiteral),
    ("string.special", K::StringLiteral),
    ("escape", K::StringLiteralEscape),
    ("string.escape", K::StringLiteralEscape),
    ("number", K::NumericLiteral),
    ("boolean", K::BooleanLiteral),
    ("keyword", K::Keyword),
    // Field names, mapping keys and section headers read as attributes,
    // matching how the deb822 emitter highlights control-field names.
    ("property", K::IdentifierAttribute),
    ("attribute", K::IdentifierAttribute),
    ("tag", K::IdentifierAttribute),
    ("type", K::IdentifierType),
    ("type.builtin", K::IdentifierType),
    ("constant", K::IdentifierConstant),
    ("constant.builtin", K::IdentifierConstant),
    ("constructor", K::IdentifierFunction),
    ("function", K::IdentifierFunction),
    ("function.builtin", K::IdentifierFunction),
    ("function.method", K::IdentifierFunction),
    ("module", K::IdentifierNamespace),
    ("embedded", K::IdentifierNamespace),
    ("variable", K::Identifier),
    ("variable.builtin", K::Identifier),
    ("variable.parameter", K::IdentifierParameter),
    // Operators and punctuation are deliberately absent: marking every brace
    // and comma adds occurrences without improving readability.
];

/// Markup grammars (markdown, rST) use a different capture vocabulary. Both the
/// older `text.*` spelling (tree-sitter-md, many Neovim queries) and the newer
/// `markup.*` one. Structural elements map to the nearest code kind.
const MARKUP: CaptureMap = &[
    ("comment", K::Comment),
    ("text.title", K::IdentifierAttribute),
    ("markup.heading", K::IdentifierAttribute),
    ("text.literal", K::StringLiteral),
    ("markup.raw", K::StringLiteral),
    ("text.uri", K::IdentifierNamespace),
    ("text.reference", K::IdentifierNamespace),
    ("markup.link", K::IdentifierNamespace),
    ("markup.link.url", K::IdentifierNamespace),
    ("string.escape", K::StringLiteralEscape),
];

/// A grammar we can index, keyed by how a file's name selects it.
struct Grammar {
    /// Display name, used as the highlight-config name.
    name: &'static str,
    language: tree_sitter::Language,
    highlights_query: &'static str,
    /// Capture-name to kind mapping for this grammar's query.
    captures: CaptureMap,
}

fn grammars() -> Vec<Grammar> {
    vec![
        Grammar {
            name: "toml",
            language: tree_sitter_toml_ng::LANGUAGE.into(),
            highlights_query: tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "json",
            language: tree_sitter_json::LANGUAGE.into(),
            highlights_query: tree_sitter_json::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "yaml",
            language: tree_sitter_yaml::LANGUAGE.into(),
            highlights_query: tree_sitter_yaml::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "make",
            language: tree_sitter_make::LANGUAGE.into(),
            highlights_query: tree_sitter_make::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        // Covers freedesktop key=value [Section] files too: .desktop entries
        // and systemd units route here.
        Grammar {
            name: "ini",
            language: tree_sitter_ini::LANGUAGE.into(),
            highlights_query: tree_sitter_ini::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "nix",
            language: tree_sitter_nix::LANGUAGE.into(),
            highlights_query: tree_sitter_nix::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "xml",
            language: tree_sitter_xml::LANGUAGE_XML.into(),
            highlights_query: tree_sitter_xml::XML_HIGHLIGHT_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "css",
            language: tree_sitter_css::LANGUAGE.into(),
            highlights_query: tree_sitter_css::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "html",
            language: tree_sitter_html::LANGUAGE.into(),
            highlights_query: tree_sitter_html::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "lua",
            language: tree_sitter_lua::LANGUAGE.into(),
            highlights_query: tree_sitter_lua::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "haskell",
            language: tree_sitter_haskell::LANGUAGE.into(),
            highlights_query: tree_sitter_haskell::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "ocaml",
            language: tree_sitter_ocaml::LANGUAGE_OCAML.into(),
            highlights_query: tree_sitter_ocaml::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        // Block grammar only: headings, code fences, list/quote markers. Inline
        // spans (emphasis, links) need a second injected pass we don't run, so
        // they render plain. The common case (README structure) still lights up.
        Grammar {
            name: "markdown",
            language: tree_sitter_md::LANGUAGE.into(),
            highlights_query: tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
            captures: MARKUP,
        },
        Grammar {
            name: "c",
            language: tree_sitter_c::LANGUAGE.into(),
            highlights_query: tree_sitter_c::HIGHLIGHT_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "cpp",
            language: tree_sitter_cpp::LANGUAGE.into(),
            highlights_query: tree_sitter_cpp::HIGHLIGHT_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "python",
            language: tree_sitter_python::LANGUAGE.into(),
            highlights_query: tree_sitter_python::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "bash",
            language: tree_sitter_bash::LANGUAGE.into(),
            highlights_query: tree_sitter_bash::HIGHLIGHT_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "java",
            language: tree_sitter_java::LANGUAGE.into(),
            highlights_query: tree_sitter_java::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "javascript",
            language: tree_sitter_javascript::LANGUAGE.into(),
            highlights_query: tree_sitter_javascript::HIGHLIGHT_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "typescript",
            language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            highlights_query: tree_sitter_typescript::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "ruby",
            language: tree_sitter_ruby::LANGUAGE.into(),
            highlights_query: tree_sitter_ruby::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "rust",
            language: tree_sitter_rust::LANGUAGE.into(),
            highlights_query: tree_sitter_rust::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "go",
            language: tree_sitter_go::LANGUAGE.into(),
            highlights_query: tree_sitter_go::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "php",
            language: tree_sitter_php::LANGUAGE_PHP.into(),
            highlights_query: tree_sitter_php::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "c-sharp",
            language: tree_sitter_c_sharp::LANGUAGE.into(),
            highlights_query: tree_sitter_c_sharp::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        // tree-sitter-vim and tree-sitter-fish expose their grammar through a
        // `language()` function rather than a `LANGUAGE` constant.
        Grammar {
            name: "vim",
            language: tree_sitter_vim::language(),
            highlights_query: tree_sitter_vim::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "scheme",
            language: tree_sitter_scheme::LANGUAGE.into(),
            highlights_query: tree_sitter_scheme::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "fortran",
            language: tree_sitter_fortran::LANGUAGE.into(),
            highlights_query: tree_sitter_fortran::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "r",
            language: tree_sitter_r::LANGUAGE.into(),
            highlights_query: tree_sitter_r::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
        Grammar {
            name: "fish",
            language: tree_sitter_fish::language(),
            highlights_query: tree_sitter_fish::HIGHLIGHTS_QUERY,
            captures: BASE,
        },
    ]
}

/// Directory names whose contents are build output or vendored dependencies,
/// not source we want to highlight. Skipped wherever they appear in the tree.
const SKIP_DIRS: &[&str] = &["target", "node_modules", ".git", "vendor"];

/// Pick the grammar name for a path by extension and well-known filenames, or
/// `None` if no grammar applies.
fn grammar_for_path(path: &Path) -> Option<&'static str> {
    let file_name = path.file_name()?.to_str()?;
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        match ext {
            "toml" => return Some("toml"),
            "json" => return Some("json"),
            "yaml" | "yml" => return Some("yaml"),
            "mk" => return Some("make"),
            "nix" => return Some("nix"),
            "xml" | "xsd" | "xsl" | "xslt" | "svg" => return Some("xml"),
            "css" => return Some("css"),
            "html" | "htm" | "xhtml" => return Some("html"),
            "lua" => return Some("lua"),
            "hs" => return Some("haskell"),
            "ml" | "mli" => return Some("ocaml"),
            "md" | "markdown" => return Some("markdown"),
            "c" => return Some("c"),
            "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => return Some("cpp"),
            // .h is C or C++; tree-sitter-c parses plain headers fine.
            "h" => return Some("c"),
            "py" | "pyi" | "pyw" => return Some("python"),
            "sh" | "bash" | "ksh" => return Some("bash"),
            "java" => return Some("java"),
            "js" | "mjs" | "cjs" => return Some("javascript"),
            "ts" => return Some("typescript"),
            "rb" => return Some("ruby"),
            "rs" => return Some("rust"),
            "go" => return Some("go"),
            "php" => return Some("php"),
            "cs" => return Some("c-sharp"),
            "vim" => return Some("vim"),
            "scm" | "ss" => return Some("scheme"),
            "f" | "f90" | "f95" | "f03" | "for" => return Some("fortran"),
            "r" => return Some("r"),
            "fish" => return Some("fish"),
            // freedesktop/systemd key=value [Section] files and generic INI.
            "ini" | "cfg" | "conf" | "desktop" | "service" | "timer" | "socket" | "mount"
            | "target" | "path" | "slice" | "scope" | "network" | "netdev" | "link" => {
                return Some("ini")
            }
            _ => {}
        }
    }
    match file_name {
        "Makefile" | "makefile" | "GNUmakefile" => Some("make"),
        // Cargo.lock is TOML despite its extension.
        "Cargo.lock" => Some("toml"),
        _ => None,
    }
}

/// Pick a grammar for an extensionless file from its `#!` line, or `None`. Only
/// interpreters for grammars we cover are mapped. Where a real indexer also
/// handles the language (scip-python, scip-shell), `--exclude-scip` lets that
/// indexer's richer tokens win, so claiming the file here is harmless.
fn grammar_for_shebang(source: &str) -> Option<&'static str> {
    let first_line = source.lines().next()?;
    let rest = first_line.strip_prefix("#!")?;
    // The interpreter is the last path component of the first shebang token,
    // accounting for "/usr/bin/env lua".
    let mut tokens = rest.split_whitespace();
    let interpreter = tokens.next()?;
    let basename = interpreter.rsplit('/').next().unwrap_or(interpreter);
    let name = if basename == "env" {
        tokens.next().unwrap_or("")
    } else {
        basename
    };
    match name {
        "lua" => Some("lua"),
        "sh" | "bash" | "dash" | "ksh" => Some("bash"),
        "fish" => Some("fish"),
        "python" | "python2" | "python3" => Some("python"),
        "ruby" => Some("ruby"),
        "Rscript" => Some("r"),
        _ => None,
    }
}

/// A configured highlighter for one grammar. Built once and reused across files.
struct LangHighlighter {
    config: HighlightConfiguration,
    /// Kind for each recognized capture, indexed by the `Highlight(usize)` the
    /// highlighter returns. That index is the position in the configured name
    /// list, so this parallels the names passed to `configure`.
    kinds: Vec<SyntaxKind>,
}

impl LangHighlighter {
    fn new(grammar: &Grammar) -> Result<Self> {
        let mut config = HighlightConfiguration::new(
            grammar.language.clone(),
            grammar.name,
            grammar.highlights_query,
            "",
            "",
        )
        .with_context(|| format!("building highlight config for {}", grammar.name))?;
        let names: Vec<&str> = grammar.captures.iter().map(|(n, _)| *n).collect();
        config.configure(&names);
        let kinds = grammar.captures.iter().map(|(_, k)| *k).collect();
        Ok(Self { config, kinds })
    }

    /// Kind for a highlight index, or `Unspecified` if out of range.
    fn kind(&self, index: usize) -> SyntaxKind {
        self.kinds
            .get(index)
            .copied()
            .unwrap_or(SyntaxKind::UnspecifiedSyntaxKind)
    }
}

/// Translate UTF-8 byte offsets to (line, UTF-16 column) positions. Built once
/// per file from its bytes.
struct PositionMap {
    /// For each line, its starting byte offset.
    line_starts: Vec<usize>,
}

impl PositionMap {
    fn new(source: &str) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self { line_starts }
    }

    /// Line index (zero-based) for a byte offset.
    fn line_of(&self, byte: usize) -> usize {
        match self.line_starts.binary_search(&byte) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    /// UTF-16 column for a byte offset, given the source it indexes.
    fn utf16_col(&self, source: &str, byte: usize) -> usize {
        let line = self.line_of(byte);
        let line_start = self.line_starts[line];
        source[line_start..byte].chars().map(char::len_utf16).sum()
    }
}

/// Highlight one file and append its occurrences to `doc`. Spans crossing a
/// line boundary are split per line so each occurrence stays single-line, which
/// the renderer requires.
fn index_file(
    highlighter: &mut Highlighter,
    lang: &LangHighlighter,
    source: &str,
    doc: &mut Document,
) -> Result<()> {
    let positions = PositionMap::new(source);
    let events = highlighter
        .highlight(&lang.config, source.as_bytes(), None, |_| None)
        .context("highlighting source")?;

    // The event stream nests highlights; the innermost active one wins for a
    // given source span. Track the stack and resolve each `Source` span against
    // its top.
    let mut stack: Vec<SyntaxKind> = Vec::new();
    for event in events {
        match event.context("highlight event")? {
            HighlightEvent::HighlightStart(h) => {
                stack.push(lang.kind(h.0));
            }
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let Some(&kind) = stack.last() else { continue };
                if kind == SyntaxKind::UnspecifiedSyntaxKind {
                    continue;
                }
                emit_span(doc, source, &positions, start, end, kind);
            }
        }
    }
    Ok(())
}

/// Emit one (possibly multi-line) byte span as one occurrence per line it
/// covers. Leading/trailing whitespace-only fragments are skipped so we don't
/// paint indentation.
fn emit_span(
    doc: &mut Document,
    source: &str,
    positions: &PositionMap,
    start: usize,
    end: usize,
    kind: SyntaxKind,
) {
    let mut line = positions.line_of(start);
    let mut seg_start = start;
    loop {
        let line_end_byte = if line + 1 < positions.line_starts.len() {
            positions.line_starts[line + 1] - 1
        } else {
            source.len()
        };
        let seg_end = end.min(line_end_byte);
        if seg_end > seg_start && !source[seg_start..seg_end].trim().is_empty() {
            let start_col = positions.utf16_col(source, seg_start);
            let end_col = positions.utf16_col(source, seg_end);
            let mut occ = Occurrence::new();
            occ.range = vec![line as i32, start_col as i32, end_col as i32];
            occ.syntax_kind = kind.into();
            doc.occurrences.push(occ);
        }
        if end <= line_end_byte {
            break;
        }
        line += 1;
        seg_start = positions.line_starts[line];
    }
}

#[derive(ClapParser)]
#[command(
    about = "Emit a SCIP index of syntax-highlighting tokens for files tree-sitter covers but language indexers don't."
)]
struct Args {
    /// Source tree to walk.
    #[arg(long)]
    root: PathBuf,
    /// Output .scip path.
    #[arg(long)]
    output: PathBuf,
    /// Existing .scip indexes to defer to: any document they already cover is
    /// skipped, so a real language indexer's richer tokens win over our
    /// syntax-only ones. Accepts several paths per flag and may be repeated.
    #[arg(long = "exclude-scip", value_name = "FILE", num_args = 1.., action = clap::ArgAction::Append)]
    exclude_scip: Vec<PathBuf>,
}

/// Collect the set of document paths covered by existing SCIP indexes, so files
/// a real indexer already handled are not re-emitted with syntax-only tokens.
fn covered_paths(scip_files: &[PathBuf]) -> Result<std::collections::HashSet<String>> {
    let mut covered = std::collections::HashSet::new();
    for file in scip_files {
        let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        let index = <Index as protobuf::Message>::parse_from_bytes(&bytes)
            .with_context(|| format!("parsing {}", file.display()))?;
        for doc in &index.documents {
            covered.insert(doc.relative_path.clone());
        }
    }
    Ok(covered)
}

fn main() -> Result<()> {
    let args = Args::parse();

    let covered = covered_paths(&args.exclude_scip)?;

    let grammar_list = grammars();
    let mut highlighters: std::collections::HashMap<&str, LangHighlighter> =
        std::collections::HashMap::new();
    for g in &grammar_list {
        highlighters.insert(g.name, LangHighlighter::new(g)?);
    }

    let mut highlighter = Highlighter::new();
    let mut index = Index::new();
    let mut meta = Metadata::new();
    meta.version = ProtocolVersion::UnspecifiedProtocolVersion.into();
    let mut tool = ToolInfo::new();
    tool.name = "scip-tree-sitter".into();
    tool.version = env!("CARGO_PKG_VERSION").into();
    meta.tool_info = protobuf::MessageField::some(tool);
    meta.project_root = format!("file://{}", args.root.display());
    meta.text_document_encoding = TextEncoding::UTF8.into();
    index.metadata = protobuf::MessageField::some(meta);

    let walker = ignore::WalkBuilder::new(&args.root)
        .hidden(false)
        .filter_entry(|entry| {
            !(entry.file_type().is_some_and(|t| t.is_dir())
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| SKIP_DIRS.contains(&name)))
        })
        .build();
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        // Extension and well-known filenames are enough for most files; an
        // extensionless file may still declare its language via a shebang, which
        // needs the file's first line, so that case is resolved after reading.
        let grammar_by_path = grammar_for_path(path);
        if grammar_by_path.is_none() && path.extension().is_some() {
            continue;
        }

        let relative = path
            .strip_prefix(&args.root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        // Defer to any indexer that already covered this document.
        if covered.contains(&relative) {
            continue;
        }

        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let Ok(source) = String::from_utf8(bytes) else {
            continue;
        };

        let Some(grammar_name) = grammar_by_path.or_else(|| grammar_for_shebang(&source)) else {
            continue;
        };

        let mut doc = Document::new();
        doc.relative_path = relative;
        doc.language = grammar_name.to_string();

        let lang = highlighters
            .get(grammar_name)
            .expect("grammar registered above");
        index_file(&mut highlighter, lang, &source, &mut doc)?;

        if !doc.occurrences.is_empty() {
            index.documents.push(doc);
        }
    }

    let bytes = protobuf::Message::write_to_bytes(&index).context("serializing SCIP index")?;
    std::fs::write(&args.output, bytes)
        .with_context(|| format!("writing {}", args.output.display()))?;
    eprintln!(
        "scip-tree-sitter: indexed {} file(s) into {}",
        index.documents.len(),
        args.output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(map: CaptureMap, name: &str) -> Option<SyntaxKind> {
        map.iter().find(|(n, _)| *n == name).map(|(_, k)| *k)
    }

    #[test]
    fn base_map_covers_common_captures() {
        assert_eq!(lookup(BASE, "comment"), Some(SyntaxKind::Comment));
        assert_eq!(lookup(BASE, "string"), Some(SyntaxKind::StringLiteral));
        assert_eq!(lookup(BASE, "boolean"), Some(SyntaxKind::BooleanLiteral));
        assert_eq!(
            lookup(BASE, "property"),
            Some(SyntaxKind::IdentifierAttribute)
        );
        assert_eq!(
            lookup(BASE, "variable.parameter"),
            Some(SyntaxKind::IdentifierParameter)
        );
        // Operators and punctuation are deliberately absent.
        assert_eq!(lookup(BASE, "operator"), None);
        assert_eq!(lookup(BASE, "punctuation.bracket"), None);
    }

    #[test]
    fn markup_map_covers_structure() {
        assert_eq!(
            lookup(MARKUP, "text.title"),
            Some(SyntaxKind::IdentifierAttribute)
        );
        assert_eq!(
            lookup(MARKUP, "markup.heading"),
            Some(SyntaxKind::IdentifierAttribute)
        );
        assert_eq!(
            lookup(MARKUP, "text.literal"),
            Some(SyntaxKind::StringLiteral)
        );
        assert_eq!(
            lookup(MARKUP, "text.uri"),
            Some(SyntaxKind::IdentifierNamespace)
        );
    }

    #[test]
    fn capture_maps_have_no_duplicate_names() {
        for (label, map) in [("BASE", BASE), ("MARKUP", MARKUP)] {
            let mut names: Vec<&str> = map.iter().map(|(n, _)| *n).collect();
            names.sort_unstable();
            let before = names.len();
            names.dedup();
            assert_eq!(names.len(), before, "{label} has a duplicate capture name");
        }
    }

    #[test]
    fn every_grammar_builds() {
        // Each grammar's query compiles against its capture-name list.
        for g in grammars() {
            LangHighlighter::new(&g)
                .unwrap_or_else(|e| panic!("grammar {} failed to build: {e}", g.name));
        }
    }

    #[test]
    fn selects_grammar_by_name() {
        assert_eq!(grammar_for_path(Path::new("Cargo.toml")), Some("toml"));
        // Cargo.lock is TOML despite the .lock extension.
        assert_eq!(grammar_for_path(Path::new("Cargo.lock")), Some("toml"));
        assert_eq!(grammar_for_path(Path::new("a/b.json")), Some("json"));
        assert_eq!(grammar_for_path(Path::new("ci.yml")), Some("yaml"));
        assert_eq!(grammar_for_path(Path::new("x.yaml")), Some("yaml"));
        assert_eq!(grammar_for_path(Path::new("setup.sh")), Some("bash"));
        // debian/rules is Makefile syntax but has no extension to key on.
        assert_eq!(grammar_for_path(Path::new("debian/rules")), None);
        assert_eq!(grammar_for_path(Path::new("Makefile")), Some("make"));
        assert_eq!(grammar_for_path(Path::new("GNUmakefile")), Some("make"));
        assert_eq!(grammar_for_path(Path::new("flake.nix")), Some("nix"));
        assert_eq!(grammar_for_path(Path::new("doc.xml")), Some("xml"));
        assert_eq!(grammar_for_path(Path::new("icon.svg")), Some("xml"));
        assert_eq!(grammar_for_path(Path::new("setup.cfg")), Some("ini"));
        // freedesktop/systemd units route through the INI grammar.
        assert_eq!(grammar_for_path(Path::new("app.desktop")), Some("ini"));
        assert_eq!(grammar_for_path(Path::new("foo.service")), Some("ini"));
        assert_eq!(grammar_for_path(Path::new("style.css")), Some("css"));
        assert_eq!(grammar_for_path(Path::new("page.html")), Some("html"));
        assert_eq!(grammar_for_path(Path::new("init.lua")), Some("lua"));
        assert_eq!(grammar_for_path(Path::new("Main.hs")), Some("haskell"));
        assert_eq!(grammar_for_path(Path::new("lib.ml")), Some("ocaml"));
        assert_eq!(grammar_for_path(Path::new("lib.mli")), Some("ocaml"));
        assert_eq!(grammar_for_path(Path::new("README.md")), Some("markdown"));
        assert_eq!(grammar_for_path(Path::new("main.c")), Some("c"));
        assert_eq!(grammar_for_path(Path::new("util.h")), Some("c"));
        assert_eq!(grammar_for_path(Path::new("widget.cpp")), Some("cpp"));
        assert_eq!(grammar_for_path(Path::new("widget.hpp")), Some("cpp"));
        assert_eq!(grammar_for_path(Path::new("app.py")), Some("python"));
        assert_eq!(grammar_for_path(Path::new("Main.java")), Some("java"));
        assert_eq!(grammar_for_path(Path::new("index.js")), Some("javascript"));
        assert_eq!(grammar_for_path(Path::new("index.ts")), Some("typescript"));
        assert_eq!(grammar_for_path(Path::new("lib.rs")), Some("rust"));
        assert_eq!(grammar_for_path(Path::new("main.go")), Some("go"));
        assert_eq!(grammar_for_path(Path::new("app.rb")), Some("ruby"));
        assert_eq!(grammar_for_path(Path::new("index.php")), Some("php"));
        assert_eq!(grammar_for_path(Path::new("Program.cs")), Some("c-sharp"));
        assert_eq!(grammar_for_path(Path::new("plugin.vim")), Some("vim"));
        assert_eq!(grammar_for_path(Path::new("list.scm")), Some("scheme"));
        assert_eq!(grammar_for_path(Path::new("calc.f90")), Some("fortran"));
        assert_eq!(grammar_for_path(Path::new("plot.r")), Some("r"));
        assert_eq!(grammar_for_path(Path::new("config.fish")), Some("fish"));
        assert_eq!(grammar_for_path(Path::new("LICENSE")), None);
    }

    #[test]
    fn selects_grammar_by_shebang() {
        // An extensionless script declares its language via the shebang.
        assert_eq!(
            grammar_for_shebang("#!/usr/bin/lua\nprint(1)\n"),
            Some("lua")
        );
        assert_eq!(grammar_for_shebang("#!/usr/bin/env lua\n"), Some("lua"));
        assert_eq!(grammar_for_shebang("#!/bin/sh\n"), Some("bash"));
        assert_eq!(grammar_for_shebang("#!/bin/bash\n"), Some("bash"));
        assert_eq!(
            grammar_for_shebang("#!/usr/bin/env python3\n"),
            Some("python")
        );
        // Perl has no grammar wired in, so its shebang yields nothing.
        assert_eq!(grammar_for_shebang("#!/usr/bin/perl\n"), None);
        // Files with no shebang yield nothing.
        assert_eq!(grammar_for_shebang("print(1)\n"), None);
    }

    #[test]
    fn utf16_columns_count_code_units() {
        // The second line holds a 'ĳ' (one UTF-16 unit, two UTF-8 bytes), so a
        // byte offset past it must still map to its UTF-16 column.
        let src = "ab\nx\u{0133}y\n";
        let positions = PositionMap::new(src);
        let line2 = src.find('x').unwrap();
        assert_eq!(positions.line_of(line2), 1);
        // 'x' is column 0, 'ĳ' is column 1, 'y' starts at column 2 even though
        // it is byte 3 within the line.
        let y = src.rfind('y').unwrap();
        assert_eq!(positions.utf16_col(src, y), 2);
    }

    #[test]
    fn splits_multiline_spans_per_line() {
        // A single byte span covering two lines yields one occurrence per line,
        // each trimmed of the trailing newline.
        let src = "foo\nbar\n";
        let positions = PositionMap::new(src);
        let mut doc = Document::new();
        emit_span(&mut doc, src, &positions, 0, src.len(), SyntaxKind::Comment);
        let ranges: Vec<&Vec<i32>> = doc.occurrences.iter().map(|o| &o.range).collect();
        assert_eq!(ranges, vec![&vec![0, 0, 3], &vec![1, 0, 3]]);
    }

    #[test]
    fn skips_whitespace_only_fragments() {
        // The leading-indentation fragment of a span is dropped.
        let src = "  x\n";
        let positions = PositionMap::new(src);
        let mut doc = Document::new();
        emit_span(&mut doc, src, &positions, 0, 3, SyntaxKind::Comment);
        assert_eq!(doc.occurrences.len(), 1);

        let mut blank = Document::new();
        emit_span(
            &mut blank,
            "   \n",
            &PositionMap::new("   \n"),
            0,
            3,
            SyntaxKind::Comment,
        );
        assert!(blank.occurrences.is_empty());
    }

    /// Run one grammar over a source string and return the kinds it emits,
    /// asserting every occurrence is symbol-less and single-line.
    fn kinds_for(grammar_name: &str, source: &str) -> Vec<SyntaxKind> {
        let grammar = grammars()
            .into_iter()
            .find(|g| g.name == grammar_name)
            .expect("grammar exists");
        let lang = LangHighlighter::new(&grammar).unwrap();
        let mut highlighter = Highlighter::new();
        let mut doc = Document::new();
        index_file(&mut highlighter, &lang, source, &mut doc).unwrap();
        for occ in &doc.occurrences {
            assert!(occ.symbol.is_empty(), "syntax tokens carry no symbol");
            assert_eq!(occ.range.len(), 3, "single-line range");
        }
        doc.occurrences
            .iter()
            .map(|o| o.syntax_kind.enum_value().unwrap())
            .collect()
    }

    #[test]
    fn highlights_toml_end_to_end() {
        let kinds = kinds_for("toml", "# c\nkey = \"val\"\n");
        assert!(kinds.contains(&SyntaxKind::Comment));
        assert!(kinds.contains(&SyntaxKind::IdentifierAttribute));
        assert!(kinds.contains(&SyntaxKind::StringLiteral));
    }

    #[test]
    fn highlights_markdown_structure() {
        // Headings map to attribute, fenced/indented code to string, via the
        // MARKUP capture map rather than BASE.
        let kinds = kinds_for("markdown", "# Title\n\n    code\n");
        assert!(kinds.contains(&SyntaxKind::IdentifierAttribute));
        assert!(kinds.contains(&SyntaxKind::StringLiteral));
    }

    #[test]
    fn highlights_haskell_via_next_capture() {
        // The Haskell highlighter drives tree-sitter's next_capture path, which
        // segfaulted on tree-sitter 0.25 for large files (shellcheck's
        // Analytics.hs). This guards the grammar wiring on the runtime we pin.
        let kinds = kinds_for("haskell", "module M where\nf x = x + 1\n");
        assert!(kinds.contains(&SyntaxKind::Keyword));
        assert!(kinds.contains(&SyntaxKind::NumericLiteral));
    }

    #[test]
    fn highlights_c_end_to_end() {
        let kinds = kinds_for("c", "/* c */\nint main(void) { return 0; }\n");
        assert!(kinds.contains(&SyntaxKind::Comment));
        assert!(kinds.contains(&SyntaxKind::IdentifierType));
        assert!(kinds.contains(&SyntaxKind::NumericLiteral));
    }

    #[test]
    fn highlights_python_end_to_end() {
        let kinds = kinds_for("python", "# c\nx = \"s\"\ndef f():\n    return 1\n");
        assert!(kinds.contains(&SyntaxKind::Comment));
        assert!(kinds.contains(&SyntaxKind::StringLiteral));
        assert!(kinds.contains(&SyntaxKind::Keyword));
    }

    #[test]
    fn highlights_vim_via_language_fn() {
        // tree-sitter-vim is wired through its `language()` function rather than
        // a `LANGUAGE` constant, so this also guards that path.
        let kinds = kinds_for("vim", "\" comment\nlet x = 1\n");
        assert!(kinds.contains(&SyntaxKind::Comment));
        assert!(kinds.contains(&SyntaxKind::NumericLiteral));
    }
}
