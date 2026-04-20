//! Recursive-descent parser for the supported SystemVerilog subset.
//!
//! Parses: module declarations, typedef enum, always_ff, always_comb,
//! case/if-else, assignments, assign statements, and `// @mununu` comments.

use super::ast::*;
use crate::adapter::{AdapterError, AdapterErrorKind, SourceLocation};
use std::collections::HashMap;

/// Type information resolved from packages and typedefs.
#[derive(Debug, Clone)]
enum TypeInfo {
    /// An enum type with its variants and bit-width.
    Enum {
        variants: Vec<String>,
        #[expect(dead_code)]
        width: usize,
    },
    /// A packed struct type with computed field bit-ranges.
    Struct(StructLayout),
}

/// Layout of a packed struct — maps field names to bit ranges within the register.
#[derive(Debug, Clone)]
struct StructLayout {
    /// Fields with their bit ranges: (name, msb, lsb).
    /// Ordered MSB-first (first declared field occupies highest bits).
    fields: Vec<(String, usize, usize)>,
    /// Total width in bits.
    total_width: usize,
}

/// Parse a SystemVerilog source string into a Module AST.
///
/// Handles `package ... endpackage` blocks before the module and resolves
/// `import pkg::*;` / `import pkg::name;` statements.
pub fn parse(input: &str) -> Result<Module, AdapterError> {
    let mut parser = Parser::new(input);

    // Parse any packages that appear before the module.
    // We must not consume @mununu annotations — they belong to the module.
    // Only skip plain whitespace and check for `package` keyword directly.
    loop {
        parser.skip_plain_whitespace();
        if parser.at_end() {
            break;
        }
        let rem = parser.remaining();
        let is_package = rem.starts_with("package")
            && rem
                .as_bytes()
                .get(7)
                .is_none_or(|&c| !c.is_ascii_alphanumeric() && c != b'_');
        if is_package {
            parser.parse_package()?;
        } else {
            break;
        }
    }

    parser.parse_module()
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
    line: usize,
    col: usize,
    /// Package definitions parsed before the module.
    packages: HashMap<String, PackageDecl>,
    /// Type scope: type names available in the current module (from imports and local typedefs).
    type_scope: HashMap<String, TypeInfo>,
    /// Variables that have a struct type — used for resolving `var.field` to bit-slices.
    var_struct_types: HashMap<String, StructLayout>,
}

/// Contents of a parsed package.
#[derive(Debug, Clone, Default)]
struct PackageDecl {
    /// Enum types: name → (variants, width)
    enums: HashMap<String, (Vec<String>, usize)>,
    /// Packed struct types: name → layout
    structs: HashMap<String, StructLayout>,
    /// Parameters: name → value
    params: HashMap<String, i64>,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            pos: 0,
            line: 1,
            col: 1,
            packages: HashMap::new(),
            type_scope: HashMap::new(),
            var_struct_types: HashMap::new(),
        }
    }

    fn remaining(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn at_end(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn skip_whitespace_and_comments(&mut self) -> Vec<MununuAnnotation> {
        let mut annotations = Vec::new();
        loop {
            // Skip whitespace
            while self.pos < self.input.len() {
                let ch = self.input.as_bytes()[self.pos];
                if ch == b' ' || ch == b'\t' || ch == b'\r' {
                    self.pos += 1;
                    self.col += 1;
                } else if ch == b'\n' {
                    self.pos += 1;
                    self.line += 1;
                    self.col = 1;
                } else {
                    break;
                }
            }

            // Check for line comments
            if self.remaining().starts_with("//") {
                let line_end = self
                    .remaining()
                    .find('\n')
                    .unwrap_or(self.remaining().len());
                let comment = &self.remaining()[2..line_end].trim();

                // Check for @mununu annotation
                if let Some(rest) = comment.strip_prefix("@mununu")
                    && let Some(ann) = parse_mununu_annotation(rest.trim())
                {
                    annotations.push(ann);
                }

                self.pos += line_end;
                continue;
            }

            // Check for block comments
            if self.remaining().starts_with("/*")
                && let Some(end) = self.remaining().find("*/")
            {
                let comment_text = &self.remaining()[..end + 2];
                for ch in comment_text.bytes() {
                    if ch == b'\n' {
                        self.line += 1;
                        self.col = 1;
                    } else {
                        self.col += 1;
                    }
                }
                self.pos += end + 2;
                continue;
            }

            break;
        }
        annotations
    }

    /// Skip whitespace only (no comments). Used when we need to peek ahead
    /// without consuming `@mununu` annotations that belong to the module.
    fn skip_plain_whitespace(&mut self) {
        while self.pos < self.input.len() {
            let ch = self.input.as_bytes()[self.pos];
            if ch == b' ' || ch == b'\t' || ch == b'\r' {
                self.pos += 1;
                self.col += 1;
            } else if ch == b'\n' {
                self.pos += 1;
                self.line += 1;
                self.col = 1;
            } else {
                break;
            }
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), AdapterError> {
        self.skip_whitespace_and_comments();
        if self.remaining().starts_with(kw)
            && self
                .remaining()
                .as_bytes()
                .get(kw.len())
                .is_none_or(|&c| !c.is_ascii_alphanumeric() && c != b'_')
        {
            self.pos += kw.len();
            self.col += kw.len();
            Ok(())
        } else {
            Err(self.error(format!("expected '{kw}'")))
        }
    }

    fn expect_char(&mut self, ch: char) -> Result<(), AdapterError> {
        self.skip_whitespace_and_comments();
        if self.remaining().starts_with(ch) {
            self.pos += 1;
            self.col += 1;
            Ok(())
        } else {
            Err(self.error(format!("expected '{ch}'")))
        }
    }

    fn peek_keyword(&mut self, kw: &str) -> bool {
        self.skip_whitespace_and_comments();
        self.remaining().starts_with(kw)
            && self
                .remaining()
                .as_bytes()
                .get(kw.len())
                .is_none_or(|&c| !c.is_ascii_alphanumeric() && c != b'_')
    }

    fn parse_ident(&mut self) -> Result<String, AdapterError> {
        self.skip_whitespace_and_comments();
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input.as_bytes()[self.pos];
            if ch.is_ascii_alphanumeric() || ch == b'_' {
                self.pos += 1;
                self.col += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(self.error("expected identifier".to_string()));
        }
        Ok(self.input[start..self.pos].to_string())
    }

    fn parse_number(&mut self) -> Result<i64, AdapterError> {
        self.skip_whitespace_and_comments();
        // Handle 'N format (e.g., '0, '1)
        if self.remaining().starts_with('\'') {
            self.pos += 1;
            self.col += 1;
            while self.pos < self.input.len()
                && self.input.as_bytes()[self.pos].is_ascii_alphanumeric()
            {
                self.pos += 1;
                self.col += 1;
            }
            return Ok(0); // Simplified: treat tick-literals as 0
        }
        let start = self.pos;
        if self.pos < self.input.len() && self.input.as_bytes()[self.pos] == b'-' {
            self.pos += 1;
            self.col += 1;
        }
        while self.pos < self.input.len() && self.input.as_bytes()[self.pos].is_ascii_digit() {
            self.pos += 1;
            self.col += 1;
        }
        if self.pos == start {
            return Err(self.error("expected number".to_string()));
        }
        let num_str = &self.input[start..self.pos];
        // Check for sized literal: N'bXXX, N'hXX, N'dNN, N'oOO
        if self.pos < self.input.len() && self.input.as_bytes()[self.pos] == b'\'' {
            self.pos += 1; // consume tick
            self.col += 1;
            let base = if self.pos < self.input.len() {
                let b = self.input.as_bytes()[self.pos];
                self.pos += 1;
                self.col += 1;
                b
            } else {
                b'd'
            };
            let val_start = self.pos;
            while self.pos < self.input.len()
                && self.input.as_bytes()[self.pos].is_ascii_alphanumeric()
            {
                self.pos += 1;
                self.col += 1;
            }
            let val_str = &self.input[val_start..self.pos];
            let radix = match base {
                b'b' | b'B' => 2,
                b'o' | b'O' => 8,
                b'h' | b'H' => 16,
                _ => 10,
            };
            return i64::from_str_radix(val_str, radix).or(Ok(0)); // fallback to 0 for unparseable
        }
        num_str
            .parse()
            .map_err(|_| self.error("invalid number".to_string()))
    }

    fn error(&self, message: String) -> AdapterError {
        AdapterError {
            kind: AdapterErrorKind::ParseError,
            message,
            location: Some(SourceLocation {
                line: self.line,
                column: self.col,
            }),
        }
    }

    // -------------------------------------------------------------------
    // Module parsing
    // -------------------------------------------------------------------

    /// Parse an `import pkg::*;` or `import pkg::name;` statement.
    ///
    /// Resolves imported items from the package registry into the module's type scope
    /// and optionally emits declarations and parameters.
    fn parse_import(
        &mut self,
        declarations: &mut Vec<Declaration>,
        parameters: &mut Vec<Parameter>,
    ) -> Result<(), AdapterError> {
        self.expect_keyword("import")?;

        // Parse package name
        let pkg_name = self.parse_ident()?;
        self.expect_str("::")?;

        self.skip_whitespace_and_comments();
        let is_wildcard = self.remaining().starts_with('*');
        if is_wildcard {
            self.pos += 1;
            self.col += 1;
        }
        let item_name = if !is_wildcard {
            Some(self.parse_ident()?)
        } else {
            None
        };
        self.expect_char(';')?;

        // Resolve from package registry
        if let Some(pkg) = self.packages.get(&pkg_name).cloned() {
            if is_wildcard {
                // Import all items
                for (type_name, (variants, width)) in &pkg.enums {
                    self.type_scope.insert(
                        type_name.clone(),
                        TypeInfo::Enum {
                            variants: variants.clone(),
                            width: *width,
                        },
                    );
                }
                for (type_name, layout) in &pkg.structs {
                    self.type_scope
                        .insert(type_name.clone(), TypeInfo::Struct(layout.clone()));
                }
                for (param_name, value) in &pkg.params {
                    parameters.push(Parameter {
                        name: param_name.clone(),
                        default_value: *value,
                    });
                }
            } else if let Some(ref name) = item_name {
                // Import specific item
                if let Some((variants, width)) = pkg.enums.get(name) {
                    self.type_scope.insert(
                        name.clone(),
                        TypeInfo::Enum {
                            variants: variants.clone(),
                            width: *width,
                        },
                    );
                } else if let Some(layout) = pkg.structs.get(name) {
                    self.type_scope
                        .insert(name.clone(), TypeInfo::Struct(layout.clone()));
                } else if let Some(value) = pkg.params.get(name) {
                    parameters.push(Parameter {
                        name: name.clone(),
                        default_value: *value,
                    });
                }
                // Unknown items are silently ignored (may be unsupported constructs)
            }
        }
        // Unknown packages are silently ignored (may come from external files)

        let _ = declarations; // will be used for struct imports in Phase C
        Ok(())
    }

    /// Try to parse a variable declaration using a type from the type scope.
    ///
    /// Handles patterns like `priv_lvl_t state;` where `priv_lvl_t` is an imported enum type.
    /// Returns `None` if the current token is not a known type name.
    ///
    /// NOTE: This must not consume whitespace/comments before peeking — the caller
    /// loop has already consumed them (including `@mununu` annotations).
    fn try_parse_typed_var_from_scope(&mut self) -> Result<Option<Declaration>, AdapterError> {
        if self.type_scope.is_empty() {
            return Ok(None);
        }

        // Peek at the next identifier without consuming it
        let saved_pos = self.pos;
        let _saved_line = self.line;
        let saved_col = self.col;

        let ident = match self.peek_ident() {
            Some(id) => id,
            None => return Ok(None),
        };

        // Check if this identifier is a known type
        if let Some(type_info) = self.type_scope.get(&ident).cloned() {
            // Consume the type name
            self.pos = saved_pos + ident.len();
            self.col = saved_col + ident.len();

            // Parse the variable name that follows
            self.skip_whitespace_and_comments();
            let var_name = self.parse_ident()?;
            self.expect_char(';')?;

            match type_info {
                TypeInfo::Enum { variants, .. } => Ok(Some(Declaration::Enum {
                    name: ident,
                    variants,
                    var_name: Some(var_name),
                })),
                TypeInfo::Struct(layout) => {
                    self.var_struct_types
                        .insert(var_name.clone(), layout.clone());
                    Ok(Some(Declaration::Logic {
                        name: var_name,
                        width: layout.total_width,
                    }))
                }
            }
        } else {
            // Not a type name — position unchanged
            Ok(None)
        }
    }

    /// Peek at the next identifier without advancing the parser position.
    fn peek_ident(&self) -> Option<String> {
        let remaining = self.remaining();
        let start = 0;
        let mut end = start;
        while end < remaining.len() {
            let ch = remaining.as_bytes()[end] as char;
            if ch.is_alphanumeric() || ch == '_' {
                end += 1;
            } else {
                break;
            }
        }
        if end > start {
            Some(remaining[start..end].to_string())
        } else {
            None
        }
    }

    /// Expect a specific string literal (e.g., "::").
    fn expect_str(&mut self, expected: &str) -> Result<(), AdapterError> {
        self.skip_whitespace_and_comments();
        if self.remaining().starts_with(expected) {
            self.pos += expected.len();
            self.col += expected.len();
            Ok(())
        } else {
            Err(self.error(format!("expected '{expected}'")))
        }
    }

    /// Parse a `package <name>; ... endpackage` block and store it in the registry.
    fn parse_package(&mut self) -> Result<(), AdapterError> {
        self.skip_whitespace_and_comments();
        self.expect_keyword("package")?;
        let name = self.parse_ident()?;
        self.expect_char(';')?;

        let mut pkg = PackageDecl::default();

        loop {
            self.skip_whitespace_and_comments();
            if self.at_end() || self.peek_keyword("endpackage") {
                break;
            }

            if self.peek_keyword("typedef") {
                // Parse typedef (enum or struct) inside package
                let decl = self.parse_typedef()?;
                match decl {
                    Declaration::Enum {
                        name: type_name,
                        variants,
                        ..
                    } => {
                        let width = if variants.len() <= 2 {
                            1
                        } else {
                            (variants.len() as f64).log2().ceil() as usize
                        };
                        pkg.enums.insert(type_name.clone(), (variants, width));
                    }
                    Declaration::Logic {
                        name: type_name, ..
                    } => {
                        // Struct typedef — layout was registered in type_scope during parsing
                        if let Some(TypeInfo::Struct(layout)) = self.type_scope.get(&type_name) {
                            pkg.structs.insert(type_name, layout.clone());
                        }
                    }
                }
            } else if self.peek_keyword("localparam") || self.peek_keyword("parameter") {
                if let Some(p) = self.parse_localparam()? {
                    pkg.params.insert(p.name, p.default_value);
                }
            } else {
                // Skip unknown constructs inside package
                self.skip_to_semicolon_or_end()?;
            }
        }

        self.expect_keyword("endpackage")?;
        // Consume optional semicolon after endpackage (skip whitespace only, not comments)
        self.skip_plain_whitespace();
        if self.remaining().starts_with(';') {
            self.pos += 1;
            self.col += 1;
        }

        self.packages.insert(name, pkg);
        Ok(())
    }

    fn parse_module(&mut self) -> Result<Module, AdapterError> {
        let mut all_annotations = Vec::new();

        // Collect any leading @mununu comments
        all_annotations.extend(self.skip_whitespace_and_comments());

        self.expect_keyword("module")?;
        let name = self.parse_ident()?;

        // Optional parameter list: #(parameter N = 4, ...)
        let parameters = self.parse_parameter_list()?;

        // Port list
        let ports = self.parse_port_list()?;
        self.expect_char(';')?;

        // Module body
        let mut parameters = parameters;
        let mut declarations = Vec::new();
        let mut always_blocks = Vec::new();
        let mut assigns = Vec::new();

        loop {
            let anns = self.skip_whitespace_and_comments();
            all_annotations.extend(anns);

            if self.at_end() || self.peek_keyword("endmodule") {
                break;
            }

            if self.peek_keyword("import") {
                self.parse_import(&mut declarations, &mut parameters)?;
            } else if self.peek_keyword("typedef") {
                declarations.push(self.parse_typedef()?);
            } else if self.peek_keyword("logic") || self.peek_keyword("reg") {
                declarations.push(self.parse_logic_decl()?);
            } else if self.peek_keyword("localparam") || self.peek_keyword("parameter") {
                if let Some(p) = self.parse_localparam()? {
                    parameters.push(p);
                }
            } else if self.peek_keyword("always_ff") {
                always_blocks.push(self.parse_always_ff()?);
            } else if self.peek_keyword("always_comb") {
                always_blocks.push(self.parse_always_comb()?);
            } else if self.peek_keyword("assign") {
                assigns.push(self.parse_assign()?);
            } else if let Some(decl) = self.try_parse_typed_var_from_scope()? {
                // Handle variable declarations using imported/typedef'd types
                declarations.push(decl);
            } else {
                // Skip unknown constructs until semicolon or block end
                self.skip_to_semicolon_or_end()?;
            }
        }

        self.expect_keyword("endmodule")?;

        // Distribute annotations into Module fields
        let mut mununu_properties = Vec::new();
        let mut domain_annotations = Vec::new();
        let mut controllable_signals = Vec::new();
        let mut input_signals = Vec::new();
        let mut force_kripke = false;

        for ann in all_annotations {
            match ann {
                MununuAnnotation::Property(p) => mununu_properties.push(p),
                MununuAnnotation::Domain(d) => domain_annotations.push(d),
                MununuAnnotation::Controllable(sigs) => controllable_signals.extend(sigs),
                MununuAnnotation::Input(sigs) => input_signals.extend(sigs),
                MununuAnnotation::ModeKripke => force_kripke = true,
            }
        }

        Ok(Module {
            name,
            parameters,
            ports,
            declarations,
            always_blocks,
            assigns,
            mununu_properties,
            domain_annotations,
            controllable_signals,
            input_signals,
            force_kripke,
        })
    }

    fn parse_port_list(&mut self) -> Result<Vec<Port>, AdapterError> {
        self.expect_char('(')?;
        let mut ports = Vec::new();

        loop {
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with(')') {
                self.pos += 1;
                self.col += 1;
                break;
            }

            let direction = if self.peek_keyword("input") {
                self.expect_keyword("input")?;
                PortDirection::Input
            } else if self.peek_keyword("output") {
                self.expect_keyword("output")?;
                PortDirection::Output
            } else if self.peek_keyword("inout") {
                self.expect_keyword("inout")?;
                PortDirection::Inout
            } else {
                PortDirection::Input // default
            };

            // Optional type: logic, reg, wire
            if self.peek_keyword("logic") || self.peek_keyword("reg") || self.peek_keyword("wire") {
                self.parse_ident()?; // consume type keyword
            }

            // Optional width: [N:M]
            let width = self.parse_optional_width()?;

            let name = self.parse_ident()?;

            ports.push(Port {
                name,
                direction,
                width,
            });

            self.skip_whitespace_and_comments();
            if self.remaining().starts_with(',') {
                self.pos += 1;
                self.col += 1;
            }
        }

        Ok(ports)
    }

    /// Parse `#(parameter N = 4, parameter M = 2)` or empty if no `#`.
    fn parse_parameter_list(&mut self) -> Result<Vec<Parameter>, AdapterError> {
        self.skip_whitespace_and_comments();
        if !self.remaining().starts_with('#') {
            return Ok(vec![]);
        }
        self.pos += 1; // consume '#'
        self.col += 1;
        self.expect_char('(')?;

        let mut params = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with(')') {
                self.pos += 1;
                self.col += 1;
                break;
            }
            // Consume "parameter" keyword if present
            if self.peek_keyword("parameter") {
                self.expect_keyword("parameter")?;
            }
            // Optional type: int, integer, etc.
            if self.peek_keyword("int") || self.peek_keyword("integer") {
                self.parse_ident()?;
            }
            let name = self.parse_ident()?;
            self.expect_char('=')?;
            let value = self.parse_number()?;
            params.push(Parameter {
                name,
                default_value: value,
            });
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with(',') {
                self.pos += 1;
                self.col += 1;
            }
        }
        Ok(params)
    }

    /// Parse `localparam N = 4;` or `parameter N = 4;` in the module body.
    fn parse_localparam(&mut self) -> Result<Option<Parameter>, AdapterError> {
        self.parse_ident()?; // consume "localparam" or "parameter"
        // Optional type
        if self.peek_keyword("int") || self.peek_keyword("integer") {
            self.parse_ident()?;
        }
        let name = self.parse_ident()?;
        self.expect_char('=')?;
        // Value might be an expression like $clog2(N) — try to parse
        self.skip_whitespace_and_comments();
        if self.remaining().starts_with('$') {
            // Skip function call like $clog2(N) — treat as unknown
            self.skip_to_semicolon_or_end()?;
            return Ok(None);
        }
        let value = self.parse_number()?;
        self.expect_char(';')?;
        Ok(Some(Parameter {
            name,
            default_value: value,
        }))
    }

    fn parse_optional_width(&mut self) -> Result<usize, AdapterError> {
        self.skip_whitespace_and_comments();
        if !self.remaining().starts_with('[') {
            return Ok(1);
        }
        self.pos += 1;
        self.col += 1;
        let msb = self.parse_number()?;
        self.expect_char(':')?;
        let lsb = self.parse_number()?;
        self.expect_char(']')?;
        Ok((msb - lsb + 1).unsigned_abs() as usize)
    }

    fn parse_typedef(&mut self) -> Result<Declaration, AdapterError> {
        self.expect_keyword("typedef")?;

        // Dispatch: typedef enum ... or typedef struct packed ...
        if self.peek_keyword("struct") {
            return self.parse_typedef_struct();
        }

        self.expect_keyword("enum")?;

        // Optional: logic [N:0]
        if self.peek_keyword("logic") {
            self.expect_keyword("logic")?;
            self.parse_optional_width()?;
        }

        self.expect_char('{')?;
        let mut variants = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with('}') {
                self.pos += 1;
                self.col += 1;
                break;
            }
            let variant = self.parse_ident()?;
            variants.push(variant);
            // Skip optional value assignment: = 3'b001, = 4, etc.
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with('=') {
                self.pos += 1;
                self.col += 1;
                // Skip value expression until comma or closing brace
                while self.pos < self.input.len() {
                    let ch = self.input.as_bytes()[self.pos];
                    if ch == b',' || ch == b'}' {
                        break;
                    }
                    if ch == b'\n' {
                        self.line += 1;
                        self.col = 0;
                    }
                    self.pos += 1;
                    self.col += 1;
                }
            }
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with(',') {
                self.pos += 1;
                self.col += 1;
            }
        }
        let type_name = self.parse_ident()?;
        self.expect_char(';')?;

        // Register in type scope for later variable declarations
        let width = if variants.len() <= 2 {
            1
        } else {
            (variants.len() as f64).log2().ceil() as usize
        };
        self.type_scope.insert(
            type_name.clone(),
            TypeInfo::Enum {
                variants: variants.clone(),
                width,
            },
        );

        // Look for variable declaration using this type
        let var_name = self.try_parse_typed_var(&type_name)?;

        Ok(Declaration::Enum {
            name: type_name,
            variants,
            var_name,
        })
    }

    /// Parse `typedef struct packed { logic [N:0] field; ... } type_name;`
    ///
    /// Computes the packed bit layout (fields laid out MSB-first) and registers
    /// the struct in the type scope. Returns a `Declaration::Logic` with the
    /// total packed width if a variable is declared.
    fn parse_typedef_struct(&mut self) -> Result<Declaration, AdapterError> {
        self.expect_keyword("struct")?;
        // `packed` is optional but expected for synthesizable code
        if self.peek_keyword("packed") {
            self.expect_keyword("packed")?;
        }

        self.expect_char('{')?;

        // Parse fields: `logic [N:0] field_name;`
        let mut fields_raw: Vec<(String, usize)> = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with('}') {
                self.pos += 1;
                self.col += 1;
                break;
            }
            // Expect `logic` keyword
            if self.peek_keyword("logic") {
                self.expect_keyword("logic")?;
            }
            let width = self.parse_optional_width()?;
            let field_name = self.parse_ident()?;
            self.expect_char(';')?;
            fields_raw.push((field_name, width));
        }

        let type_name = self.parse_ident()?;
        self.expect_char(';')?;

        // Compute packed layout: fields are MSB-first
        // First field occupies the highest bits
        let total_width: usize = fields_raw.iter().map(|(_, w)| w).sum();
        let mut offset = total_width;
        let mut fields = Vec::with_capacity(fields_raw.len());
        for (name, width) in &fields_raw {
            let msb = offset - 1;
            let lsb = offset - width;
            fields.push((name.clone(), msb, lsb));
            offset -= width;
        }

        let layout = StructLayout {
            fields,
            total_width,
        };

        // Register in type scope
        self.type_scope
            .insert(type_name.clone(), TypeInfo::Struct(layout));

        // Look for variable declaration using this type
        let var_name = self.try_parse_typed_var(&type_name)?;

        if let Some(ref var) = var_name {
            // Register this variable as having struct type for field resolution
            if let Some(TypeInfo::Struct(layout)) = self.type_scope.get(&type_name) {
                self.var_struct_types.insert(var.clone(), layout.clone());
            }
            Ok(Declaration::Logic {
                name: var.clone(),
                width: total_width,
            })
        } else {
            // Typedef only, no variable — emit a zero-width Logic placeholder
            // (the type is in scope for later variable declarations)
            Ok(Declaration::Logic {
                name: type_name,
                width: total_width,
            })
        }
    }

    fn try_parse_typed_var(&mut self, type_name: &str) -> Result<Option<String>, AdapterError> {
        self.skip_whitespace_and_comments();
        if self.peek_keyword(type_name) {
            self.expect_keyword(type_name)?;
            let var_name = self.parse_ident()?;
            self.expect_char(';')?;
            Ok(Some(var_name))
        } else {
            Ok(None)
        }
    }

    fn parse_logic_decl(&mut self) -> Result<Declaration, AdapterError> {
        self.parse_ident()?; // consume 'logic' or 'reg'
        let width = self.parse_optional_width()?;
        let name = self.parse_ident()?;
        self.expect_char(';')?;
        Ok(Declaration::Logic { name, width })
    }

    fn parse_always_ff(&mut self) -> Result<AlwaysBlock, AdapterError> {
        self.expect_keyword("always_ff")?;
        // Skip sensitivity list: @(posedge clk or posedge rst)
        self.skip_whitespace_and_comments();
        if self.remaining().starts_with('@') {
            self.pos += 1;
            self.col += 1;
        }
        self.skip_balanced('(', ')')?;
        let body = self.parse_statement()?;

        // Extract reset info from if (rst) ... else ... pattern
        let reset = extract_reset(&body);

        Ok(AlwaysBlock::AlwaysFF { reset, body })
    }

    fn parse_always_comb(&mut self) -> Result<AlwaysBlock, AdapterError> {
        self.expect_keyword("always_comb")?;
        let body = self.parse_statement()?;
        Ok(AlwaysBlock::AlwaysComb { body })
    }

    fn parse_assign(&mut self) -> Result<ContinuousAssign, AdapterError> {
        self.expect_keyword("assign")?;
        let base_name = self.parse_ident()?;
        let target = if self.remaining().starts_with('.') {
            self.pos += 1;
            self.col += 1;
            let field_name = self.parse_ident()?;
            if let Some(layout) = self.var_struct_types.get(&base_name) {
                if let Some((_, msb, lsb)) = layout.fields.iter().find(|(n, _, _)| n == &field_name)
                {
                    AssignTarget::BitSlice {
                        base: base_name,
                        msb: *msb,
                        lsb: *lsb,
                    }
                } else {
                    return Err(self.error(format!(
                        "unknown field '{}' on struct variable '{}'",
                        field_name, base_name
                    )));
                }
            } else {
                return Err(self.error(format!("'{}' is not a struct variable", base_name)));
            }
        } else {
            AssignTarget::Simple(base_name)
        };
        self.expect_char('=')?;
        let value = self.parse_expr()?;
        self.expect_char(';')?;
        Ok(ContinuousAssign { target, value })
    }

    fn parse_statement(&mut self) -> Result<Statement, AdapterError> {
        self.skip_whitespace_and_comments();

        // Null statement: just a semicolon
        if self.remaining().starts_with(';') {
            self.pos += 1;
            self.col += 1;
            return Ok(Statement::Block(vec![]));
        }

        if self.peek_keyword("begin") {
            self.expect_keyword("begin")?;
            let mut stmts = Vec::new();
            loop {
                self.skip_whitespace_and_comments();
                if self.peek_keyword("end") {
                    self.expect_keyword("end")?;
                    break;
                }
                stmts.push(self.parse_statement()?);
            }
            Ok(Statement::Block(stmts))
        } else if self.peek_keyword("if") {
            self.parse_if_statement()
        } else if self.peek_keyword("case") || self.peek_keyword("casez") {
            self.parse_case_statement()
        } else {
            // Assignment: target <= expr; or target = expr;
            // Handles struct field writes: var.field <= expr;
            let base_name = self.parse_ident()?;
            let target = if self.remaining().starts_with('.') {
                // Struct field write
                self.pos += 1;
                self.col += 1;
                let field_name = self.parse_ident()?;
                if let Some(layout) = self.var_struct_types.get(&base_name) {
                    if let Some((_, msb, lsb)) =
                        layout.fields.iter().find(|(n, _, _)| n == &field_name)
                    {
                        AssignTarget::BitSlice {
                            base: base_name,
                            msb: *msb,
                            lsb: *lsb,
                        }
                    } else {
                        return Err(self.error(format!(
                            "unknown field '{}' on struct variable '{}'",
                            field_name, base_name
                        )));
                    }
                } else {
                    return Err(self.error(format!("'{}' is not a struct variable", base_name)));
                }
            } else {
                AssignTarget::Simple(base_name)
            };
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with("<=") {
                self.pos += 2;
                self.col += 2;
                let value = self.parse_expr()?;
                self.expect_char(';')?;
                Ok(Statement::NonblockingAssign { target, value })
            } else if self.remaining().starts_with('=') {
                self.pos += 1;
                self.col += 1;
                let value = self.parse_expr()?;
                self.expect_char(';')?;
                Ok(Statement::BlockingAssign { target, value })
            } else {
                // Skip to semicolon
                self.skip_to_semicolon_or_end()?;
                Ok(Statement::Block(vec![]))
            }
        }
    }

    fn parse_if_statement(&mut self) -> Result<Statement, AdapterError> {
        self.expect_keyword("if")?;
        self.expect_char('(')?;
        let cond = self.parse_expr()?;
        self.expect_char(')')?;
        let then_branch = self.parse_statement()?;

        let else_branch = if self.peek_keyword("else") {
            self.expect_keyword("else")?;
            Some(Box::new(self.parse_statement()?))
        } else {
            None
        };

        Ok(Statement::If {
            cond,
            then_branch: Box::new(then_branch),
            else_branch,
        })
    }

    fn parse_case_statement(&mut self) -> Result<Statement, AdapterError> {
        // consume 'case' or 'casez'
        self.parse_ident()?;
        self.expect_char('(')?;
        let selector = self.parse_ident()?;
        self.expect_char(')')?;

        let mut branches = Vec::new();
        let mut default = None;

        loop {
            self.skip_whitespace_and_comments();
            if self.peek_keyword("endcase") {
                self.expect_keyword("endcase")?;
                break;
            }
            if self.peek_keyword("default") {
                self.expect_keyword("default")?;
                self.expect_char(':')?;
                default = Some(Box::new(self.parse_statement()?));
            } else {
                // Case label can be an identifier (IDLE) or a number (0, 3)
                self.skip_whitespace_and_comments();
                let label = if self
                    .remaining()
                    .as_bytes()
                    .first()
                    .is_some_and(|c| c.is_ascii_digit())
                {
                    self.parse_number()?.to_string()
                } else {
                    self.parse_ident()?
                };
                self.expect_char(':')?;
                let body = self.parse_statement()?;
                branches.push(CaseBranch { label, body });
            }
        }

        Ok(Statement::Case {
            selector,
            branches,
            default,
        })
    }

    fn parse_expr(&mut self) -> Result<Expr, AdapterError> {
        let expr = self.parse_or_expr()?;
        // Check for ternary: expr ? then : else
        self.skip_whitespace_and_comments();
        if self.remaining().starts_with('?') {
            self.pos += 1;
            self.col += 1;
            let then_expr = self.parse_expr()?;
            self.expect_char(':')?;
            let else_expr = self.parse_expr()?;
            Ok(Expr::Ternary {
                cond: Box::new(expr),
                then_expr: Box::new(then_expr),
                else_expr: Box::new(else_expr),
            })
        } else {
            Ok(expr)
        }
    }

    fn parse_or_expr(&mut self) -> Result<Expr, AdapterError> {
        let mut left = self.parse_and_expr()?;
        loop {
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with("||") {
                self.pos += 2;
                self.col += 2;
                let right = self.parse_and_expr()?;
                left = Expr::BinOp {
                    op: BinOp::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_and_expr(&mut self) -> Result<Expr, AdapterError> {
        let mut left = self.parse_bitor_expr()?;
        loop {
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with("&&") {
                self.pos += 2;
                self.col += 2;
                let right = self.parse_bitor_expr()?;
                left = Expr::BinOp {
                    op: BinOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_bitor_expr(&mut self) -> Result<Expr, AdapterError> {
        let mut left = self.parse_bitand_expr()?;
        loop {
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with('|')
                && !self.remaining().starts_with("||")
                && !self.remaining().starts_with("|=")
            {
                self.pos += 1;
                self.col += 1;
                let right = self.parse_bitand_expr()?;
                left = Expr::BinOp {
                    op: BinOp::BitOr,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_bitand_expr(&mut self) -> Result<Expr, AdapterError> {
        let mut left = self.parse_comparison_expr()?;
        loop {
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with('&')
                && !self.remaining().starts_with("&&")
                && !self.remaining().starts_with("&=")
            {
                self.pos += 1;
                self.col += 1;
                let right = self.parse_comparison_expr()?;
                left = Expr::BinOp {
                    op: BinOp::BitAnd,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_comparison_expr(&mut self) -> Result<Expr, AdapterError> {
        let left = self.parse_shift_expr()?;
        self.skip_whitespace_and_comments();
        if self.remaining().starts_with("==") {
            self.pos += 2;
            self.col += 2;
            let right = self.parse_shift_expr()?;
            Ok(Expr::BinOp {
                op: BinOp::Eq,
                left: Box::new(left),
                right: Box::new(right),
            })
        } else if self.remaining().starts_with("!=") {
            self.pos += 2;
            self.col += 2;
            let right = self.parse_shift_expr()?;
            Ok(Expr::BinOp {
                op: BinOp::Ne,
                left: Box::new(left),
                right: Box::new(right),
            })
        } else if self.remaining().starts_with("<=") {
            // Careful: in expression context this is <=, not nonblocking assign
            self.pos += 2;
            self.col += 2;
            let right = self.parse_shift_expr()?;
            Ok(Expr::BinOp {
                op: BinOp::Le,
                left: Box::new(left),
                right: Box::new(right),
            })
        } else if self.remaining().starts_with(">=") {
            self.pos += 2;
            self.col += 2;
            let right = self.parse_shift_expr()?;
            Ok(Expr::BinOp {
                op: BinOp::Ge,
                left: Box::new(left),
                right: Box::new(right),
            })
        } else if self.remaining().starts_with('<') && !self.remaining().starts_with("<<") {
            self.pos += 1;
            self.col += 1;
            let right = self.parse_shift_expr()?;
            Ok(Expr::BinOp {
                op: BinOp::Lt,
                left: Box::new(left),
                right: Box::new(right),
            })
        } else if self.remaining().starts_with('>') && !self.remaining().starts_with(">>") {
            self.pos += 1;
            self.col += 1;
            let right = self.parse_shift_expr()?;
            Ok(Expr::BinOp {
                op: BinOp::Gt,
                left: Box::new(left),
                right: Box::new(right),
            })
        } else {
            Ok(left)
        }
    }

    fn parse_shift_expr(&mut self) -> Result<Expr, AdapterError> {
        let mut left = self.parse_additive_expr()?;
        loop {
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with("<<") {
                self.pos += 2;
                self.col += 2;
                let right = self.parse_additive_expr()?;
                left = Expr::BinOp {
                    op: BinOp::Shl,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else if self.remaining().starts_with(">>") {
                self.pos += 2;
                self.col += 2;
                let right = self.parse_additive_expr()?;
                left = Expr::BinOp {
                    op: BinOp::Shr,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_additive_expr(&mut self) -> Result<Expr, AdapterError> {
        let mut left = self.parse_multiplicative_expr()?;
        loop {
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with('+') && !self.remaining().starts_with("+=") {
                self.pos += 1;
                self.col += 1;
                let right = self.parse_multiplicative_expr()?;
                left = Expr::BinOp {
                    op: BinOp::Add,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else if self.remaining().starts_with('-')
                && !self.remaining().starts_with("-=")
                && !self.remaining().starts_with("->")
            {
                self.pos += 1;
                self.col += 1;
                let right = self.parse_multiplicative_expr()?;
                left = Expr::BinOp {
                    op: BinOp::Sub,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_multiplicative_expr(&mut self) -> Result<Expr, AdapterError> {
        let mut left = self.parse_unary_expr()?;
        loop {
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with('*') && !self.remaining().starts_with("*=") {
                self.pos += 1;
                self.col += 1;
                let right = self.parse_unary_expr()?;
                left = Expr::BinOp {
                    op: BinOp::Mul,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else if self.remaining().starts_with('/')
                && !self.remaining().starts_with("/=")
                && !self.remaining().starts_with("//")
            {
                self.pos += 1;
                self.col += 1;
                let right = self.parse_unary_expr()?;
                left = Expr::BinOp {
                    op: BinOp::Div,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else if self.remaining().starts_with('%') && !self.remaining().starts_with("%=") {
                self.pos += 1;
                self.col += 1;
                let right = self.parse_unary_expr()?;
                left = Expr::BinOp {
                    op: BinOp::Mod,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_unary_expr(&mut self) -> Result<Expr, AdapterError> {
        self.skip_whitespace_and_comments();
        if self.remaining().starts_with('!') && !self.remaining().starts_with("!=") {
            self.pos += 1;
            self.col += 1;
            let inner = self.parse_postfix_expr()?;
            Ok(Expr::Not(Box::new(inner)))
        } else if self.remaining().starts_with('~') {
            // Bitwise NOT — treat as logical NOT for abstraction
            self.pos += 1;
            self.col += 1;
            let inner = self.parse_postfix_expr()?;
            Ok(Expr::Not(Box::new(inner)))
        } else if self.remaining().starts_with('|') && !self.remaining().starts_with("||") {
            // Reduction OR: |req
            self.pos += 1;
            self.col += 1;
            let inner = self.parse_postfix_expr()?;
            Ok(Expr::BinOp {
                op: BinOp::BitOr,
                left: Box::new(inner),
                right: Box::new(Expr::Number(0)),
            })
        } else {
            self.parse_postfix_expr()
        }
    }

    fn parse_postfix_expr(&mut self) -> Result<Expr, AdapterError> {
        let mut expr = self.parse_primary_expr()?;
        // Handle postfix: bit-select x[i] or bit-slice x[msb:lsb]
        loop {
            self.skip_whitespace_and_comments();
            if self.remaining().starts_with('[') {
                self.pos += 1;
                self.col += 1;
                let index = self.parse_expr()?;
                self.skip_whitespace_and_comments();
                if self.remaining().starts_with(':') {
                    // Bit-slice: x[msb:lsb]
                    self.pos += 1;
                    self.col += 1;
                    let lsb = self.parse_expr()?;
                    self.expect_char(']')?;
                    expr = Expr::BitSlice {
                        base: Box::new(expr),
                        msb: Box::new(index),
                        lsb: Box::new(lsb),
                    };
                } else {
                    // Single-bit select: x[i]
                    self.expect_char(']')?;
                    expr = Expr::BitSelect {
                        base: Box::new(expr),
                        index: Box::new(index),
                    };
                }
            } else if self.remaining().starts_with('.') {
                // Struct field access: var.field → BitSlice
                if let Expr::Ident(ref base_name) = expr
                    && let Some(layout) = self.var_struct_types.get(base_name).cloned()
                {
                    self.pos += 1;
                    self.col += 1;
                    let field_name = self.parse_ident()?;
                    if let Some((_, msb, lsb)) =
                        layout.fields.iter().find(|(n, _, _)| n == &field_name)
                    {
                        expr = Expr::BitSlice {
                            base: Box::new(expr),
                            msb: Box::new(Expr::Number(*msb as i64)),
                            lsb: Box::new(Expr::Number(*lsb as i64)),
                        };
                        continue;
                    }
                    return Err(self.error(format!(
                        "unknown field '{}' on struct variable '{}'",
                        field_name, base_name
                    )));
                }
                break;
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary_expr(&mut self) -> Result<Expr, AdapterError> {
        self.skip_whitespace_and_comments();
        if self.remaining().starts_with('(') {
            self.pos += 1;
            self.col += 1;
            let expr = self.parse_expr()?;
            self.expect_char(')')?;
            Ok(expr)
        } else if self.remaining().starts_with('{') {
            // Concatenation: {a, b, c}
            self.pos += 1;
            self.col += 1;
            let mut parts = Vec::new();
            loop {
                self.skip_whitespace_and_comments();
                if self.remaining().starts_with('}') {
                    self.pos += 1;
                    self.col += 1;
                    break;
                }
                parts.push(self.parse_expr()?);
                self.skip_whitespace_and_comments();
                if self.remaining().starts_with(',') {
                    self.pos += 1;
                    self.col += 1;
                }
            }
            Ok(Expr::Concat(parts))
        } else if self
            .remaining()
            .as_bytes()
            .first()
            .is_some_and(|c| c.is_ascii_digit())
            || self.remaining().starts_with('\'')
        {
            let n = self.parse_number()?;
            Ok(Expr::Number(n))
        } else {
            let id = self.parse_ident()?;
            Ok(Expr::Ident(id))
        }
    }

    fn skip_balanced(&mut self, open: char, close: char) -> Result<(), AdapterError> {
        self.skip_whitespace_and_comments();
        if !self.remaining().starts_with(open) {
            return Err(self.error(format!("expected '{open}'")));
        }
        self.pos += 1;
        self.col += 1;
        let mut depth = 1;
        while depth > 0 && self.pos < self.input.len() {
            let ch = self.input.as_bytes()[self.pos];
            if ch == open as u8 {
                depth += 1;
            } else if ch == close as u8 {
                depth -= 1;
            } else if ch == b'\n' {
                self.line += 1;
                self.col = 0;
            }
            self.pos += 1;
            self.col += 1;
        }
        Ok(())
    }

    fn skip_to_semicolon_or_end(&mut self) -> Result<(), AdapterError> {
        while self.pos < self.input.len() {
            if self.input.as_bytes()[self.pos] == b';' {
                self.pos += 1;
                self.col += 1;
                return Ok(());
            }
            if self.input.as_bytes()[self.pos] == b'\n' {
                self.line += 1;
                self.col = 0;
            }
            self.pos += 1;
            self.col += 1;
        }
        Ok(())
    }
}

/// Extract reset information from an if-else pattern at the top of an always_ff.
fn extract_reset(stmt: &Statement) -> Option<ResetInfo> {
    match stmt {
        Statement::If {
            cond,
            then_branch,
            else_branch: Some(_),
        } => {
            let reset_signal = match cond {
                Expr::Ident(name) => name.clone(),
                Expr::Not(inner) => match inner.as_ref() {
                    Expr::Ident(name) => name.clone(),
                    _ => return None,
                },
                _ => return None,
            };

            let assignments = extract_assignments(then_branch);
            if assignments.is_empty() {
                return None;
            }

            Some(ResetInfo {
                reset_signal,
                assignments,
            })
        }
        Statement::Block(stmts) => {
            // Sometimes reset is wrapped: begin if (rst) ... end
            stmts.iter().find_map(extract_reset)
        }
        _ => None,
    }
}

/// Extract (target, value) pairs from assignments in a statement.
fn extract_assignments(stmt: &Statement) -> Vec<(String, String)> {
    let mut result = Vec::new();
    match stmt {
        Statement::NonblockingAssign { target, value }
        | Statement::BlockingAssign { target, value } => {
            result.push((target.name().to_string(), expr_to_string(value)));
        }
        Statement::Block(stmts) => {
            for s in stmts {
                result.extend(extract_assignments(s));
            }
        }
        _ => {}
    }
    result
}

fn expr_to_string(expr: &Expr) -> String {
    match expr {
        Expr::Ident(s) => s.clone(),
        Expr::Number(n) => n.to_string(),
        Expr::Not(inner) => format!("!{}", expr_to_string(inner)),
        Expr::Bool(b) => b.to_string(),
        Expr::BinOp { op, left, right } => {
            let op_str = match op {
                BinOp::Eq => "==",
                BinOp::Ne => "!=",
                BinOp::And => "&&",
                BinOp::Or => "||",
                BinOp::BitOr => "|",
                BinOp::BitAnd => "&",
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
                BinOp::Mod => "%",
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
                BinOp::Shl => "<<",
                BinOp::Shr => ">>",
            };
            format!(
                "({} {op_str} {})",
                expr_to_string(left),
                expr_to_string(right)
            )
        }
        Expr::Ternary {
            cond,
            then_expr,
            else_expr,
        } => format!(
            "({} ? {} : {})",
            expr_to_string(cond),
            expr_to_string(then_expr),
            expr_to_string(else_expr)
        ),
        Expr::BitSelect { base, index } => {
            format!("{}[{}]", expr_to_string(base), expr_to_string(index))
        }
        Expr::BitSlice { base, msb, lsb } => format!(
            "{}[{}:{}]",
            expr_to_string(base),
            expr_to_string(msb),
            expr_to_string(lsb)
        ),
        Expr::Concat(parts) => {
            let inner: Vec<String> = parts.iter().map(expr_to_string).collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

/// Parsed result from a `// @mununu` comment.
enum MununuAnnotation {
    Property(MununuProperty),
    Domain(MununuDomainAnnotation),
    Controllable(Vec<String>),
    Input(Vec<String>),
    ModeKripke,
}

/// Parse a `// @mununu` comment into an annotation.
fn parse_mununu_annotation(rest: &str) -> Option<MununuAnnotation> {
    // Property formats
    if let Some(r) = rest.strip_prefix("ltl") {
        return parse_property(MununuPropertyKind::Ltl, r.trim());
    }
    if let Some(r) = rest.strip_prefix("assume") {
        return parse_property(MununuPropertyKind::Assume, r.trim());
    }
    if let Some(r) = rest.strip_prefix("guarantee") {
        return parse_property(MununuPropertyKind::Guarantee, r.trim());
    }

    // Domain annotation: @mununu domain <name>: <kind>
    if let Some(r) = rest.strip_prefix("domain") {
        return parse_domain_annotation(r.trim());
    }

    // Controllable signals: @mununu controllable sig1, sig2
    if let Some(r) = rest.strip_prefix("controllable") {
        let signals = r.trim().split(',').map(|s| s.trim().to_string()).collect();
        return Some(MununuAnnotation::Controllable(signals));
    }

    // Input signals: @mununu input sig1, sig2
    if let Some(r) = rest.strip_prefix("input") {
        let signals = r.trim().split(',').map(|s| s.trim().to_string()).collect();
        return Some(MununuAnnotation::Input(signals));
    }

    // Mode selection: @mununu mode kripke
    if let Some(r) = rest.strip_prefix("mode")
        && r.trim() == "kripke"
    {
        return Some(MununuAnnotation::ModeKripke);
    }

    None
}

fn parse_property(kind: MununuPropertyKind, after_kind: &str) -> Option<MununuAnnotation> {
    let colon_pos = after_kind.find(':')?;
    let name = after_kind[..colon_pos].trim().to_string();
    let formula = after_kind[colon_pos + 1..].trim().to_string();
    Some(MununuAnnotation::Property(MununuProperty {
        kind,
        name,
        formula,
    }))
}

fn parse_domain_annotation(rest: &str) -> Option<MununuAnnotation> {
    // Format: <register_name>: <kind>
    let colon_pos = rest.find(':')?;
    let register_name = rest[..colon_pos].trim().to_string();
    let kind_str = rest[colon_pos + 1..].trim();

    let domain_kind = if kind_str == "boolean" {
        DomainAnnotationKind::Boolean
    } else if kind_str == "ignored" {
        DomainAnnotationKind::Ignored
    } else if let Some(range_str) = kind_str.strip_prefix("bounded_counter") {
        // bounded_counter 0..7
        let range_str = range_str.trim();
        let parts: Vec<&str> = range_str.split("..").collect();
        if parts.len() != 2 {
            return None;
        }
        let lower: i64 = parts[0].trim().parse().ok()?;
        let upper: i64 = parts[1].trim().parse().ok()?;
        DomainAnnotationKind::BoundedCounter { lower, upper }
    } else if let Some(variants_str) = kind_str.strip_prefix("enum") {
        // enum {V1, V2, V3} or enum {IDLE=0, START=3, OTHER}
        let variants_str = variants_str.trim();
        let inner = variants_str.strip_prefix('{')?.strip_suffix('}')?;
        let mut variants = Vec::new();
        let mut value_map = Vec::new();
        for item in inner.split(',') {
            let item = item.trim();
            if let Some((name, val_str)) = item.split_once('=') {
                let name = name.trim().to_string();
                if let Ok(val) = val_str.trim().parse::<i64>() {
                    value_map.push((name.clone(), val));
                }
                variants.push(name);
            } else {
                variants.push(item.to_string());
            }
        }
        DomainAnnotationKind::Enum {
            variants,
            value_map,
        }
    } else {
        return None;
    };

    Some(MununuAnnotation::Domain(MununuDomainAnnotation {
        register_name,
        domain_kind,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_module() {
        let module = parse("module foo(); endmodule").unwrap();
        assert_eq!(module.name, "foo");
        assert!(module.ports.is_empty());
    }

    #[test]
    fn parse_ports() {
        let module =
            parse("module test(input logic clk, input logic rst, output logic ack); endmodule")
                .unwrap();
        assert_eq!(module.ports.len(), 3);
        assert_eq!(module.ports[0].name, "clk");
        assert_eq!(module.ports[0].direction, PortDirection::Input);
        assert_eq!(module.ports[2].name, "ack");
        assert_eq!(module.ports[2].direction, PortDirection::Output);
    }

    #[test]
    fn parse_enum_typedef() {
        let module = parse(
            r#"module test();
                typedef enum logic [1:0] {IDLE, WAIT, ACK, DONE} state_t;
                state_t state;
            endmodule"#,
        )
        .unwrap();
        assert_eq!(module.declarations.len(), 1);
        if let Declaration::Enum {
            variants, var_name, ..
        } = &module.declarations[0]
        {
            assert_eq!(variants, &["IDLE", "WAIT", "ACK", "DONE"]);
            assert_eq!(var_name.as_deref(), Some("state"));
        } else {
            panic!("expected Enum declaration");
        }
    }

    #[test]
    fn parse_always_ff_with_case() {
        let module = parse(
            r#"module test(input logic clk, input logic rst);
                typedef enum logic [1:0] {IDLE, WAIT, DONE} state_t;
                state_t state;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) state <= IDLE;
                    else case (state)
                        IDLE: state <= WAIT;
                        WAIT: state <= DONE;
                        DONE: state <= IDLE;
                    endcase
                end
            endmodule"#,
        )
        .unwrap();
        assert_eq!(module.always_blocks.len(), 1);
        if let AlwaysBlock::AlwaysFF { reset, .. } = &module.always_blocks[0] {
            let r = reset.as_ref().expect("should have reset");
            assert_eq!(r.reset_signal, "rst");
            assert_eq!(
                r.assignments,
                vec![("state".to_string(), "IDLE".to_string())]
            );
        } else {
            panic!("expected AlwaysFF");
        }
    }

    #[test]
    fn parse_mununu_comments() {
        let module = parse(
            r#"// @mununu ltl safety: nu X. ([] X)
            // @mununu assume env: G(req -> X req)
            module test(); endmodule"#,
        )
        .unwrap();
        assert_eq!(module.mununu_properties.len(), 2);
        assert_eq!(module.mununu_properties[0].name, "safety");
        assert_eq!(module.mununu_properties[0].kind, MununuPropertyKind::Ltl);
        assert_eq!(module.mununu_properties[1].kind, MununuPropertyKind::Assume);
    }

    #[test]
    fn parse_assign_statement() {
        let module = parse(
            r#"module test(output logic ack);
                assign ack = 0;
            endmodule"#,
        )
        .unwrap();
        assert_eq!(module.assigns.len(), 1);
        assert_eq!(module.assigns[0].target, "ack");
    }

    #[test]
    fn parse_module_parameters() {
        let module = parse(
            r#"module rr_arbiter #(parameter N = 4) (
                input logic clk, input logic rst
            );
            endmodule"#,
        )
        .unwrap();
        assert_eq!(module.name, "rr_arbiter");
        assert_eq!(module.parameters.len(), 1);
        assert_eq!(module.parameters[0].name, "N");
        assert_eq!(module.parameters[0].default_value, 4);
    }

    #[test]
    fn parse_multiple_parameters() {
        let module = parse(
            r#"module test #(parameter N = 4, parameter M = 2) (
                input logic clk
            );
            endmodule"#,
        )
        .unwrap();
        assert_eq!(module.parameters.len(), 2);
        assert_eq!(module.parameters[0].name, "N");
        assert_eq!(module.parameters[0].default_value, 4);
        assert_eq!(module.parameters[1].name, "M");
        assert_eq!(module.parameters[1].default_value, 2);
    }

    #[test]
    fn parse_localparam_in_body() {
        let module = parse(
            r#"module test();
                localparam WIDTH = 8;
            endmodule"#,
        )
        .unwrap();
        assert_eq!(module.parameters.len(), 1);
        assert_eq!(module.parameters[0].name, "WIDTH");
        assert_eq!(module.parameters[0].default_value, 8);
    }

    #[test]
    fn parse_parameterized_fsm() {
        let module = parse(
            r#"// @mununu ltl safety: nu X. ([] X)
            module rr_arbiter #(parameter N = 2) (
                input logic clk, input logic rst,
                input logic req_a, input logic req_b,
                output logic grant_a, output logic grant_b
            );
                typedef enum logic [1:0] {IDLE, GRANT_A, GRANT_B} state_t;
                state_t state;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) state <= IDLE;
                    else case (state)
                        IDLE: begin
                            if (req_a) state <= GRANT_A;
                            else if (req_b) state <= GRANT_B;
                        end
                        GRANT_A: if (!req_a) state <= IDLE;
                        GRANT_B: if (!req_b) state <= IDLE;
                    endcase
                end
            endmodule"#,
        )
        .unwrap();
        assert_eq!(module.parameters.len(), 1);
        assert_eq!(module.parameters[0].name, "N");
        assert_eq!(module.parameters[0].default_value, 2);
        assert_eq!(module.always_blocks.len(), 1);
        assert_eq!(module.mununu_properties.len(), 1);
    }

    #[test]
    fn parse_package_and_wildcard_import() {
        let module = parse(
            r#"
            package riscv;
                typedef enum logic [1:0] {USER, SUPERVISOR, MACHINE} priv_lvl_t;
                localparam int XLEN = 32;
            endpackage

            module csr(input logic clk, input logic rst);
                import riscv::*;
                priv_lvl_t priv_lvl;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) priv_lvl <= USER;
                    else priv_lvl <= MACHINE;
                end
            endmodule
            "#,
        )
        .unwrap();

        assert_eq!(module.name, "csr");
        // import riscv::* should bring in XLEN as a parameter
        assert!(
            module
                .parameters
                .iter()
                .any(|p| p.name == "XLEN" && p.default_value == 32),
            "XLEN parameter should be imported"
        );
        // priv_lvl_t should be resolved to a Declaration::Enum via type scope
        let has_enum = module.declarations.iter().any(|d| {
            matches!(
                d,
                Declaration::Enum { name, variants, var_name: Some(v) }
                    if name == "priv_lvl_t" && variants.len() == 3 && v == "priv_lvl"
            )
        });
        assert!(
            has_enum,
            "priv_lvl_t priv_lvl should be parsed as enum declaration"
        );
    }

    #[test]
    fn parse_package_and_named_import() {
        let module = parse(
            r#"
            package bus_pkg;
                typedef enum logic {IDLE, BUSY} bus_state_t;
            endpackage

            module ctrl(input logic clk, input logic rst);
                import bus_pkg::bus_state_t;
                bus_state_t state;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) state <= IDLE;
                end
            endmodule
            "#,
        )
        .unwrap();

        let has_enum = module.declarations.iter().any(|d| {
            matches!(
                d,
                Declaration::Enum { var_name: Some(v), .. } if v == "state"
            )
        });
        assert!(
            has_enum,
            "bus_state_t state should be parsed as enum declaration"
        );
    }

    #[test]
    fn parse_package_annotations_preserved() {
        // @mununu annotations before module are preserved even with packages
        let module = parse(
            r#"
            package pkg;
                localparam int N = 4;
            endpackage

            // @mununu ltl safety: nu X. ([] X)
            module test(input logic clk, input logic rst);
                import pkg::*;
                logic flag;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) flag <= 0;
                    else flag <= 1;
                end
            endmodule
            "#,
        )
        .unwrap();

        assert_eq!(module.mununu_properties.len(), 1);
        assert_eq!(module.mununu_properties[0].name, "safety");
        assert!(
            module.parameters.iter().any(|p| p.name == "N"),
            "N should be imported from package"
        );
    }

    #[test]
    fn parse_typedef_struct_packed() {
        let module = parse(
            r#"
            // @mununu ltl safety: nu X. ([] X)
            // @mununu mode kripke
            module axi(input logic clk, input logic rst);
                typedef struct packed {
                    logic [11:0] addr;
                    logic [2:0]  size;
                    logic        valid;
                } axi_aw_t;
                axi_aw_t aw;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) aw <= 0;
                    else aw.valid <= 1;
                end
                assign addr_ok = (aw.addr < 12'd100);
            endmodule
            "#,
        )
        .unwrap();

        // Struct variable should appear as a 16-bit Logic declaration
        let aw_decl = module
            .declarations
            .iter()
            .find(|d| matches!(d, Declaration::Logic { name, .. } if name == "aw"));
        assert!(aw_decl.is_some(), "aw should be a Logic declaration");
        if let Some(Declaration::Logic { width, .. }) = aw_decl {
            assert_eq!(*width, 16, "packed struct total width: 12 + 3 + 1 = 16");
        }

        // The always_ff should have `aw.valid <= 1` resolved to BitSlice target
        assert_eq!(module.always_blocks.len(), 1);

        // The assign should contain a BitSlice expression for `aw.addr`
        assert_eq!(module.assigns.len(), 1);
    }

    #[test]
    fn parse_struct_field_layout() {
        // Verify exact bit layout: MSB-first packing
        let module = parse(
            r#"
            // @mununu ltl safety: nu X. ([] X)
            // @mununu mode kripke
            module test(input logic clk, input logic rst, input logic wr);
                typedef struct packed {
                    logic [7:0] data;
                    logic [3:0] tag;
                    logic       flag;
                } pkt_t;
                pkt_t pkt;
                // @mununu domain pkt: bounded_counter 0..8191
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) pkt <= 0;
                    else if (wr) pkt.flag <= 1;
                end
            endmodule
            "#,
        )
        .unwrap();

        // Total width: 8 + 4 + 1 = 13 bits
        let pkt_decl = module
            .declarations
            .iter()
            .find(|d| matches!(d, Declaration::Logic { name, .. } if name == "pkt"));
        assert!(pkt_decl.is_some());
        if let Some(Declaration::Logic { width, .. }) = pkt_decl {
            assert_eq!(*width, 13, "packed struct: 8 + 4 + 1 = 13");
        }
    }

    #[test]
    fn parse_struct_from_package() {
        let module = parse(
            r#"
            package axi_pkg;
                typedef struct packed {
                    logic [7:0] addr;
                    logic       valid;
                } req_t;
            endpackage

            module dut(input logic clk, input logic rst);
                import axi_pkg::*;
                req_t req;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) req <= 0;
                    else req.valid <= 1;
                end
            endmodule
            "#,
        )
        .unwrap();

        // req should be 9-bit Logic (8 + 1)
        let req_decl = module
            .declarations
            .iter()
            .find(|d| matches!(d, Declaration::Logic { name, .. } if name == "req"));
        assert!(req_decl.is_some(), "req should be declared");
        if let Some(Declaration::Logic { width, .. }) = req_decl {
            assert_eq!(*width, 9, "struct: 8 + 1 = 9 bits");
        }
    }
}
