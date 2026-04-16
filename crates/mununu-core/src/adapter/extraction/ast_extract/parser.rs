//! Tree-sitter parser initialization and language dispatch.
//!
//! Provides a unified interface for parsing TypeScript, Python, and Rust
//! source files into tree-sitter syntax trees. Language is detected from
//! file extension or explicit config.

use tree_sitter::{Language, Parser, Tree};

/// Supported source languages for AST extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLanguage {
    TypeScript,
    Python,
    Rust,
}

impl SourceLanguage {
    /// Detect language from file extension.
    pub fn from_extension(path: &str) -> Option<Self> {
        let ext = path.rsplit('.').next()?;
        match ext {
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(Self::TypeScript),
            "py" | "pyi" => Some(Self::Python),
            "rs" => Some(Self::Rust),
            _ => None,
        }
    }

    /// Detect language from explicit name string.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "typescript" | "ts" | "javascript" | "js" => Some(Self::TypeScript),
            "python" | "py" => Some(Self::Python),
            "rust" | "rs" => Some(Self::Rust),
            _ => None,
        }
    }
}

/// Parsed source file with tree-sitter AST.
pub struct ParsedSource {
    /// The source code text.
    pub source: String,
    /// The tree-sitter syntax tree.
    pub tree: Tree,
    /// Detected language.
    pub language: SourceLanguage,
}

impl ParsedSource {
    /// Get the source text for a tree-sitter node.
    pub fn node_text(&self, node: &tree_sitter::Node) -> &str {
        node.utf8_text(self.source.as_bytes()).unwrap_or("")
    }

    /// Get the source line (1-indexed) for a tree-sitter node.
    pub fn node_line(&self, node: &tree_sitter::Node) -> u32 {
        node.start_position().row as u32 + 1
    }

    /// Get the end line (1-indexed) for a tree-sitter node.
    pub fn node_end_line(&self, node: &tree_sitter::Node) -> u32 {
        node.end_position().row as u32 + 1
    }
}

/// Get the tree-sitter Language for a source language.
fn get_ts_language(lang: SourceLanguage) -> Language {
    match lang {
        SourceLanguage::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        SourceLanguage::Python => tree_sitter_python::LANGUAGE.into(),
        SourceLanguage::Rust => tree_sitter_rust::LANGUAGE.into(),
    }
}

/// Parse a source file into a tree-sitter syntax tree.
pub fn parse_source(source: &str, language: SourceLanguage) -> Result<ParsedSource, String> {
    let ts_language = get_ts_language(language);
    let mut parser = Parser::new();
    parser
        .set_language(&ts_language)
        .map_err(|e| format!("Failed to set tree-sitter language: {e}"))?;

    let tree = parser
        .parse(source, None)
        .ok_or("Tree-sitter parse returned None")?;

    if tree.root_node().has_error() {
        // Parse succeeded but tree contains error nodes — warn but continue
        // (tree-sitter is error-tolerant; partial extraction is still useful)
    }

    Ok(ParsedSource {
        source: source.to_string(),
        tree,
        language,
    })
}

/// Parse a source file from a path, detecting language from extension.
pub fn parse_file(
    path: &std::path::Path,
    language_override: Option<SourceLanguage>,
) -> Result<ParsedSource, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read '{}': {e}", path.display()))?;

    let language = language_override
        .or_else(|| path.to_str().and_then(SourceLanguage::from_extension))
        .ok_or_else(|| {
            format!(
                "Cannot detect language for '{}'. Use --language or a recognized extension.",
                path.display()
            )
        })?;

    parse_source(&source, language)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_language_from_extension() {
        assert_eq!(
            SourceLanguage::from_extension("server.ts"),
            Some(SourceLanguage::TypeScript)
        );
        assert_eq!(
            SourceLanguage::from_extension("handler.py"),
            Some(SourceLanguage::Python)
        );
        assert_eq!(
            SourceLanguage::from_extension("connection/mod.rs"),
            Some(SourceLanguage::Rust)
        );
        assert_eq!(SourceLanguage::from_extension("readme.md"), None);
    }

    #[test]
    fn detect_language_from_name() {
        assert_eq!(
            SourceLanguage::from_name("typescript"),
            Some(SourceLanguage::TypeScript)
        );
        assert_eq!(
            SourceLanguage::from_name("Python"),
            Some(SourceLanguage::Python)
        );
        assert_eq!(
            SourceLanguage::from_name("rust"),
            Some(SourceLanguage::Rust)
        );
    }

    #[test]
    fn parse_typescript_source() {
        let source = r#"
class Server {
    private _started: boolean = false;
    private _closed: boolean = false;

    start(): void {
        if (this._started) {
            throw new Error('already started');
        }
        this._started = true;
    }

    close(): void {
        if (this._closed) {
            return;
        }
        this._closed = true;
    }
}
"#;
        let parsed = parse_source(source, SourceLanguage::TypeScript).unwrap();
        assert_eq!(parsed.language, SourceLanguage::TypeScript);
        assert!(!parsed.tree.root_node().has_error());
    }

    #[test]
    fn parse_python_source() {
        let source = r#"
class Handler:
    def __init__(self):
        self._active = False
        self._count = 0

    def activate(self):
        if not self._active:
            self._active = True

    def process(self):
        if self._active:
            self._count += 1
"#;
        let parsed = parse_source(source, SourceLanguage::Python).unwrap();
        assert_eq!(parsed.language, SourceLanguage::Python);
        assert!(!parsed.tree.root_node().has_error());
    }

    #[test]
    fn parse_rust_source() {
        let source = r#"
pub struct Connection {
    started: bool,
    closed: bool,
}

impl Connection {
    pub fn start(&mut self) {
        if !self.started {
            self.started = true;
        }
    }

    pub fn close(&mut self) {
        if !self.closed {
            self.closed = true;
        }
    }
}
"#;
        let parsed = parse_source(source, SourceLanguage::Rust).unwrap();
        assert_eq!(parsed.language, SourceLanguage::Rust);
        assert!(!parsed.tree.root_node().has_error());
    }

    #[test]
    fn node_line_numbers() {
        let source = "class Foo {\n    bar(): void {}\n}\n";
        let parsed = parse_source(source, SourceLanguage::TypeScript).unwrap();
        let root = parsed.tree.root_node();
        // Root starts at line 1
        assert_eq!(parsed.node_line(&root), 1);
    }
}
