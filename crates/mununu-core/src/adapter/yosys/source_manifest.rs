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

    // Which basenames are `` `include ``d by some file → fragments, not units.
    let mut included: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (_, content) in files {
        for t in include_targets(content) {
            included.insert(t);
        }
    }

    // Compilation units: files that declare ≥1 module AND are not `` `include ``d.
    let mut units: Vec<(&PathBuf, &String, Vec<String>)> = Vec::new();
    let mut fragment_dirs: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    for (path, content) in files {
        let base = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let mods = declared_modules(content);
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
            units.push((path, content, mods));
        }
    }

    // Include dirs: the parents of every unit (so their `` `include ``s resolve)
    // plus every fragment dir.
    let mut include_dirs: std::collections::BTreeSet<PathBuf> = fragment_dirs;
    for (path, _, _) in &units {
        if let Some(d) = path.parent() {
            include_dirs.insert(d.to_path_buf());
        }
    }

    // Top = a declared module no OTHER unit instantiates.
    let all_content: String = units
        .iter()
        .map(|(_, c, _)| c.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let mut top_candidates: Vec<(String, &PathBuf)> = Vec::new();
    for (path, _, mods) in &units {
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
            .find(|(_, _, mods)| mods.contains(t))
            .map(|(p, _, _)| (*p).clone())
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
            .and_then(|tp| units.iter().position(|(p, _, _)| *p == tp))
            .unwrap_or(0);
        let primary = named(units[idx].0, units[idx].1);
        let additional = units
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != idx)
            .map(|(_, (p, c, _))| named(p, c))
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
}
