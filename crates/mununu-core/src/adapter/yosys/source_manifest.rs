//! E6 — automated per-design SV source-manifest resolver.
//!
//! Given a set of SystemVerilog files (or a directory), work out the three things
//! a multi-file lift needs so the caller does NOT have to hand-assemble them:
//!
//! - the **compilation units** (`module`-declaring files that are not themselves
//!   `` `include ``d by another file — those are fragments, resolved via `-I`);
//! - the **include-search directories** (so `` `include "frag.vh" `` resolves against
//!   the original tree instead of being compiled in isolation, which fails with
//!   `unexpected TOK_ELSE/TOK_ENDMODULE`); and
//! - the **top module** — a declared module that no other compilation unit
//!   *instantiates* (declared-minus-instantiated).
//!
//! This replaces the manual `--source`/`--include-dir`/`--top` assembly the
//! multi-file AssertLLM2 corpus needed (see [`super::YosysOptions::additional_sources`]).
//! It is a *convenience*, not a verification step: a wrong guess yields a lift
//! error (caught) or a model the caller can override with `--top`; it never
//! affects the soundness of a verdict. Detection is a lightweight token scan (no
//! full SV parse) — conservative on instantiation (a missed instantiation only
//! widens the top-candidate set, which then falls back to a name heuristic).

use std::path::{Path, PathBuf};

/// The assembled inputs for a multi-file SV lift.
#[derive(Debug, Clone)]
pub struct AssembledDesign {
    /// The primary compilation unit `(display-name, content)` — the file that
    /// declares the chosen top (or the first unit when the top is ambiguous).
    pub primary: (String, String),
    /// The remaining compilation units `(display-name, content)`.
    pub additional: Vec<(String, String)>,
    /// Include-search directories (parents of the input files), forwarded as `-I`.
    pub include_dirs: Vec<PathBuf>,
    /// The auto-detected top module (declared-minus-instantiated) when unambiguous.
    pub top: Option<String>,
    /// Human-readable notes on the decisions (for `--verbose` / diagnostics).
    pub notes: Vec<String>,
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// Blank out `//`-line and `/* */`-block comments (replacing them with whitespace so
/// byte offsets and line structure are preserved) BEFORE any `module`/instantiation
/// token scan. Without this, prose after `module` in a comment — `// the PWM module.
/// Output is …` — is mis-read as a declared module name (`Output`, `is`, `body`),
/// which then becomes a spurious un-instantiated top candidate. String literals are
/// preserved (a `//`/`/*` inside `"…"` is not a comment), so `` `include "x.vh" ``
/// targets survive; a `` `include `` that is itself commented out is correctly dropped.
/// Verilog block comments do not nest.
fn strip_comments(content: &str) -> String {
    #[derive(PartialEq)]
    enum St {
        Normal,
        Str,
        Line,
        Block,
    }
    let b = content.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut st = St::Normal;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let c2 = if i + 1 < b.len() { b[i + 1] } else { 0 };
        match st {
            St::Normal => {
                if c == b'"' {
                    st = St::Str;
                    out.push(c);
                    i += 1;
                } else if c == b'/' && c2 == b'/' {
                    st = St::Line;
                    out.push(b' ');
                    out.push(b' ');
                    i += 2;
                } else if c == b'/' && c2 == b'*' {
                    st = St::Block;
                    out.push(b' ');
                    out.push(b' ');
                    i += 2;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            St::Str => {
                // A backslash escape (`\"`) does not close the string.
                if c == b'\\' && i + 1 < b.len() {
                    out.push(c);
                    out.push(b[i + 1]);
                    i += 2;
                } else {
                    if c == b'"' {
                        st = St::Normal;
                    }
                    out.push(c);
                    i += 1;
                }
            }
            St::Line => {
                if c == b'\n' {
                    st = St::Normal;
                    out.push(c);
                } else {
                    out.push(b' ');
                }
                i += 1;
            }
            St::Block => {
                if c == b'*' && c2 == b'/' {
                    st = St::Normal;
                    out.push(b' ');
                    out.push(b' ');
                    i += 2;
                } else {
                    // Keep newlines so line numbers are preserved; blank everything else.
                    out.push(if c == b'\n' { b'\n' } else { b' ' });
                    i += 1;
                }
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Replace the CONTENTS of `"…"` string literals with spaces (run on already
/// comment-stripped text). Used for the `module`/instantiation scan so a string such as
/// `$display("module foo started")` cannot inject a fake module name; the strings-KEPT
/// [`strip_comments`] output is used separately for `` `include "target" `` extraction.
fn blank_strings(content: &str) -> String {
    let b = content.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut in_str = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == b'\\' && i + 1 < b.len() {
                out.push(b' ');
                out.push(b' ');
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
                out.push(c);
            } else {
                out.push(if c == b'\n' { b'\n' } else { b' ' });
            }
        } else if c == b'"' {
            in_str = true;
            out.push(c);
        } else {
            out.push(c);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whole-word occurrences of `word` in `content` whose following text satisfies
/// `after` (called with the byte slice immediately after the word). Skips
/// occurrences that are part of a larger identifier (e.g. `endmodule` for
/// `module`, or `my_module`).
fn word_hits(content: &str, word: &str, mut after: impl FnMut(&str) -> bool) -> usize {
    let b = content.as_bytes();
    let w = word.as_bytes();
    let mut n = 0;
    let mut i = 0;
    while let Some(rel) = content[i..].find(word) {
        let pos = i + rel;
        let prev_ok = pos == 0 || !is_ident_char(b[pos - 1]);
        let end = pos + w.len();
        let next_ok = end >= b.len() || !is_ident_char(b[end]);
        if prev_ok && next_ok && after(&content[end..]) {
            n += 1;
        }
        i = pos + w.len();
    }
    n
}

/// The module names declared in `content` (`module <name>`).
fn declared_modules(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    word_hits(content, "module", |rest| {
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if !name.is_empty() {
            out.push(name);
        }
        false // count is irrelevant; we collect via the side effect
    });
    out
}

/// The `` `include "target" `` targets referenced in `content` (basenames).
fn include_targets(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = content[i..].find("`include") {
        let pos = i + rel;
        let rest = &content[pos + "`include".len()..];
        if let Some(q0) = rest.find('"')
            && let Some(q1) = rest[q0 + 1..].find('"')
        {
            let target = &rest[q0 + 1..q0 + 1 + q1];
            let base = Path::new(target)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(target);
            out.push(base.to_string());
        }
        i = pos + "`include".len();
    }
    out
}

/// Is module `m` *instantiated* anywhere in `content` (excluding its own
/// declaration site)? Conservative: matches the `m <inst> (` and `m #(` shapes;
/// a missed instantiation only widens the top-candidate set.
fn is_instantiated(content: &str, m: &str) -> bool {
    let mut found = false;
    word_hits(content, m, |rest| {
        let t = rest.trim_start();
        let leading_ws = rest.len() - t.len();
        // `m #( … ) inst ( … )` — parameterised instantiation.
        if t.starts_with('#') {
            found = true;
            return false;
        }
        // `m inst ( … )` — needs an instance identifier then `(`. Require real
        // whitespace between `m` and the instance name so `m(` (a declaration's
        // port list or an expression) is not misread as an instantiation.
        if leading_ws == 0 {
            return false;
        }
        let inst: String = t
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if inst.is_empty() {
            return false;
        }
        let after_inst = t[inst.len()..].trim_start();
        if after_inst.starts_with('(') {
            found = true;
        }
        false
    });
    found
}

/// E6 core — assemble a multi-file SV design from `(path, content)` files.
///
/// `design_name` (e.g. the directory basename) is the tie-breaker when more than
/// one module is never instantiated: the top-candidate whose file stem matches it
/// wins, else the candidate whose module name matches it, else `None` (Yosys
/// auto-detects). `files` must be non-empty.
pub fn assemble_sv_design(
    files: &[(PathBuf, String)],
    design_name: Option<&str>,
) -> AssembledDesign {
    let mut notes = Vec::new();

    // Mask every file ONCE up front so token scans never read comment prose or string
    // contents as code. Two levels: `stripped` (comments blanked, strings KEPT) feeds
    // `` `include "target" `` extraction; `masked` (strings blanked too) feeds the
    // `module`/instantiation scan (a comment or `$display("module x")` string can no
    // longer inject a fake module name). Yosys still receives the ORIGINAL content so its
    // error line numbers stay meaningful.
    let stripped: Vec<String> = files.iter().map(|(_, c)| strip_comments(c)).collect();
    let masked: Vec<String> = stripped.iter().map(|s| blank_strings(s)).collect();

    // Which basenames are `` `include ``d by some file → fragments, not units.
    let mut included: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in &stripped {
        for t in include_targets(s) {
            included.insert(t);
        }
    }

    // Compilation units: files that declare ≥1 module AND are not `` `include ``d.
    // Each carries (path, ORIGINAL content [for yosys], MASKED content [for scans], mods).
    let mut units: Vec<(&PathBuf, &String, &str, Vec<String>)> = Vec::new();
    let mut fragment_dirs: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    for ((path, content), m) in files.iter().zip(masked.iter()) {
        let base = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let mods = declared_modules(m);
        if included.contains(&base) {
            if let Some(d) = path.parent() {
                fragment_dirs.insert(d.to_path_buf());
            }
            notes.push(format!(
                "{base}: `include fragment → include-dir, not a compilation unit"
            ));
        } else if mods.is_empty() {
            // a header/define-only file that nobody `` `include ``s directly by this
            // name — keep its dir on the search path, do not compile it standalone.
            if let Some(d) = path.parent() {
                fragment_dirs.insert(d.to_path_buf());
            }
            notes.push(format!(
                "{base}: no module declaration → header/define, include-dir only"
            ));
        } else {
            units.push((path, content, m.as_str(), mods));
        }
    }

    // DEDUP `.sv`/`.v` twins: many corpora ship both a `.v` and a `.sv` copy of the same
    // module(s) (or a flattened `work.sv` duplicating per-module files). Staging both is a
    // guaranteed `duplicate definition` lift failure. Drop any later unit whose declared
    // modules are ALL already staged by an earlier unit — the input is path-sorted
    // (`discover_sv_files`), so `foo.sv` (`.sv` < `.v`) is kept and `foo.v` dropped. A unit
    // that declares at least one NEW module is always kept (never drop unique RTL).
    let mut seen_modules: std::collections::HashSet<String> = std::collections::HashSet::new();
    units.retain(|(path, _, _, mods)| {
        if mods.iter().all(|m| seen_modules.contains(m)) {
            let base = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            notes.push(format!(
                "{base}: module(s) {mods:?} already staged → skipped (.sv/.v twin / duplicate)"
            ));
            return false;
        }
        for m in mods {
            seen_modules.insert(m.clone());
        }
        true
    });

    // Include dirs: the parents of every unit (so their `` `include ``s resolve)
    // plus every fragment dir.
    let mut include_dirs: std::collections::BTreeSet<PathBuf> = fragment_dirs;
    for (path, _, _, _) in &units {
        if let Some(d) = path.parent() {
            include_dirs.insert(d.to_path_buf());
        }
    }

    // Top = a declared module no OTHER unit instantiates. Scan the STRIPPED contents so a
    // module name appearing in a comment never registers as an instantiation.
    let all_content: String = units
        .iter()
        .map(|(_, _, s, _)| *s)
        .collect::<Vec<_>>()
        .join("\n");
    let mut top_candidates: Vec<(String, &PathBuf)> = Vec::new();
    for (path, _, _, mods) in &units {
        for m in mods {
            if !is_instantiated(&all_content, m) {
                top_candidates.push((m.clone(), path));
            }
        }
    }
    let top = match top_candidates.len() {
        1 => Some(top_candidates[0].0.clone()),
        0 => {
            notes.push("no un-instantiated module found (cyclic?) → let Yosys auto-detect".into());
            None
        }
        _ => {
            // tie-break by design name against the file stem, then the module name.
            let pick = design_name.and_then(|dn| {
                top_candidates
                    .iter()
                    .find(|(_, p)| p.file_stem().and_then(|s| s.to_str()) == Some(dn))
                    .or_else(|| top_candidates.iter().find(|(m, _)| m == dn))
                    .map(|(m, _)| m.clone())
            });
            notes.push(format!(
                "{} top candidates {:?} → {}",
                top_candidates.len(),
                top_candidates.iter().map(|(m, _)| m).collect::<Vec<_>>(),
                pick.as_deref().unwrap_or("(ambiguous — Yosys auto-detect)")
            ));
            pick
        }
    };

    // Order units so the top's file is primary.
    let top_path: Option<PathBuf> = top.as_ref().and_then(|t| {
        units
            .iter()
            .find(|(_, _, _, mods)| mods.contains(t))
            .map(|(p, _, _, _)| (*p).clone())
    });
    let named = |p: &PathBuf, c: &String| {
        (
            p.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("top.sv")
                .to_string(),
            c.clone(),
        )
    };
    let (primary, additional): ((String, String), Vec<(String, String)>) = {
        let idx = top_path
            .as_ref()
            .and_then(|tp| units.iter().position(|(p, _, _, _)| *p == tp))
            .unwrap_or(0);
        let primary = named(units[idx].0, units[idx].1);
        let additional = units
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != idx)
            .map(|(_, (p, c, _, _))| named(p, c))
            .collect();
        (primary, additional)
    };

    AssembledDesign {
        primary,
        additional,
        include_dirs: include_dirs.into_iter().collect(),
        top,
        notes,
    }
}

/// Discover SV/Verilog files under `dir` (recursively), excluding common
/// non-RTL subtrees (mutations, buggy artifacts, testbenches). CLI helper for
/// `--design-dir`; the API assembles from provided source *contents* instead.
pub fn discover_sv_files(dir: &Path) -> Result<Vec<(PathBuf, String)>, String> {
    fn excluded(p: &Path) -> bool {
        p.components().any(|c| {
            let s = c.as_os_str().to_string_lossy().to_ascii_lowercase();
            matches!(
                s.as_str(),
                "mutations" | "buggy_artifacts" | "buggy" | "tb" | "testbench" | "sim" | "figures"
            )
        })
    }
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries =
            std::fs::read_dir(&d).map_err(|e| format!("read_dir {}: {e}", d.display()))?;
        for entry in entries.flatten() {
            let p = entry.path();
            if excluded(&p) {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if matches!(
                p.extension().and_then(|s| s.to_str()),
                Some("v") | Some("sv") | Some("svh") | Some("vh")
            ) {
                let content = std::fs::read_to_string(&p)
                    .map_err(|e| format!("read {}: {e}", p.display()))?;
                out.push((p, content));
            }
        }
    }
    if out.is_empty() {
        return Err(format!("no .v/.sv sources found under {}", dir.display()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, content: &str) -> (PathBuf, String) {
        (PathBuf::from(name), content.to_string())
    }

    #[test]
    fn assembles_top_plus_submodules_and_defines() {
        // A top that instantiates two submodules + a define header it `` `include ``s.
        let files = vec![
            f(
                "design/top.v",
                "`include \"defs.vh\"\nmodule top(input clk);\n  sub_a a0 (.clk(clk));\n  sub_b #(.W(8)) b0 (.clk(clk));\nendmodule\n",
            ),
            f(
                "design/include/sub_a.v",
                "module sub_a(input clk); endmodule\n",
            ),
            f(
                "design/include/sub_b.v",
                "module sub_b #(parameter W=1)(input clk); endmodule\n",
            ),
            f("design/include/defs.vh", "`define WIDTH 8\n"),
        ];
        let a = assemble_sv_design(&files, Some("top"));
        assert_eq!(
            a.top.as_deref(),
            Some("top"),
            "top = the un-instantiated module"
        );
        assert_eq!(a.primary.0, "top.v", "primary is the top's file");
        let add: Vec<&str> = a.additional.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            add.contains(&"sub_a.v") && add.contains(&"sub_b.v"),
            "submodules are units: {add:?}"
        );
        assert!(
            !add.contains(&"defs.vh"),
            "the `include'd define header is NOT a compilation unit: {add:?}"
        );
        assert!(
            a.include_dirs.iter().any(|d| d.ends_with("include")),
            "the include/ dir is on the search path: {:?}",
            a.include_dirs
        );
    }

    #[test]
    fn declaration_site_is_not_an_instantiation() {
        // `module sub(...)` must not be mistaken for an instantiation of `sub`.
        let files = vec![f("d/only.v", "module sub(input a); endmodule\n")];
        let a = assemble_sv_design(&files, Some("only"));
        assert_eq!(a.top.as_deref(), Some("sub"), "the sole module is the top");
    }

    #[test]
    fn ambiguous_top_falls_back_to_design_name() {
        // Two un-instantiated modules → tie-break by the file stem == design name.
        let files = vec![
            f("d/foo.v", "module foo; endmodule\n"),
            f("d/bar.v", "module bar; endmodule\n"),
        ];
        assert_eq!(
            assemble_sv_design(&files, Some("bar")).top.as_deref(),
            Some("bar")
        );
        assert_eq!(
            assemble_sv_design(&files, Some("zzz")).top,
            None,
            "no match → Yosys auto-detect"
        );
    }

    #[test]
    fn declared_modules_ignores_comments_and_strings() {
        // The mis-tokenization bug: `module` in a comment or string must NOT be a decl.
        let src = "// module fake_line here\n\
                   /* the module body is here; module also_fake */\n\
                   module real1(input a); endmodule\n\
                   initial $display(\"module in_a_string running\");\n\
                   module real2; endmodule\n";
        let scan = blank_strings(&strip_comments(src));
        assert_eq!(
            declared_modules(&scan),
            vec!["real1".to_string(), "real2".to_string()],
            "only real `module` declarations count — not comment prose / string contents"
        );
    }

    #[test]
    fn comment_prose_after_module_is_not_a_top_candidate() {
        // End-to-end: a design whose ONLY real module is `PWM`, but whose comments say
        // "… the PWM module. Output is …" (the exact `Output`/`is`/`body` mis-tokenization
        // seen on the AssertLLM2 pwm/PIT designs). The top must be unambiguously `PWM`.
        let files = vec![f(
            "d/PWM.v",
            "// This is the PWM module. Output is driven here.\n\
             /* module body: the down-clocking is below; module foo is fake */\n\
             module PWM(input clk, output reg pwm_out);\n  reg [1:0] state;\nendmodule\n",
        )];
        let a = assemble_sv_design(&files, Some("PWM"));
        assert_eq!(
            a.top.as_deref(),
            Some("PWM"),
            "the only real module is the top; comment words (is/body/Output/foo) are not modules"
        );
        assert!(
            !a.notes.iter().any(|n| n.contains("top candidates")),
            "no spurious ambiguity from comment prose: {:?}",
            a.notes
        );
    }

    #[test]
    fn sv_v_twin_modules_are_deduped() {
        // The duplicate-definition bug: a design ships BOTH `foo.sv` and `foo.v` for the
        // same module (path-sorted → `.sv` before `.v`). Only the `.sv` copy is staged;
        // the `.v` twin is dropped so Yosys does not see a redefinition.
        let files = vec![
            f("d/top.sv", "module top; sub s0 (); endmodule\n"),
            f("d/sub.sv", "module sub; endmodule\n"),
            f("d/sub.v", "module sub; endmodule\n"), // the `.v` twin of sub.sv
        ];
        let a = assemble_sv_design(&files, Some("top"));
        assert_eq!(a.top.as_deref(), Some("top"));
        let staged: Vec<&str> = std::iter::once(a.primary.0.as_str())
            .chain(a.additional.iter().map(|(n, _)| n.as_str()))
            .collect();
        assert!(
            staged.contains(&"sub.sv") && !staged.contains(&"sub.v"),
            "the `.sv` copy is kept and the `.v` twin dropped: {staged:?}"
        );
        assert!(
            a.notes.iter().any(|n| n.contains(".sv/.v twin")),
            "the dedup is noted: {:?}",
            a.notes
        );
    }

    #[test]
    fn unique_module_in_a_v_file_is_never_dropped() {
        // Dedup must NOT drop a `.v` file that declares a module absent from the `.sv` set.
        let files = vec![
            f("d/top.sv", "module top; a a0(); b b0(); endmodule\n"),
            f("d/a.sv", "module a; endmodule\n"),
            f("d/b.v", "module b; endmodule\n"), // unique — no `.sv` twin
        ];
        let a = assemble_sv_design(&files, Some("top"));
        let staged: Vec<&str> = std::iter::once(a.primary.0.as_str())
            .chain(a.additional.iter().map(|(n, _)| n.as_str()))
            .collect();
        assert!(staged.contains(&"b.v"), "unique .v RTL is kept: {staged:?}");
    }

    #[test]
    fn strip_comments_preserves_include_string_but_drops_commented_include() {
        // A real `` `include `` survives (its dir is on the search path); a commented-out
        // one does not create a spurious fragment.
        let files = vec![
            f(
                "d/top.v",
                "`include \"real.vh\"\n// `include \"commented.vh\"\nmodule top; endmodule\n",
            ),
            f("d/real.vh", "`define W 8\n"),
        ];
        let a = assemble_sv_design(&files, Some("top"));
        assert!(
            a.notes
                .iter()
                .any(|n| n.contains("real.vh") && n.contains("fragment")),
            "the real `include target is a fragment: {:?}",
            a.notes
        );
        assert!(
            !a.notes.iter().any(|n| n.contains("commented.vh")),
            "a commented-out `include is not detected: {:?}",
            a.notes
        );
    }
}
