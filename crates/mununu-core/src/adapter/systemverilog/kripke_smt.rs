//! SMT-based value discovery for RTL register abstraction.
//!
//! Uses z3 bitvector theory to find concrete register values that make
//! guard conditions satisfiable. This discovers significant values hidden
//! behind combinational logic (e.g., `y = x * 4; if (y == 12)` → `x = 3`).
//!
//! Z3 is a mandatory dependency since Phase A.4 step 4.2 — the historical
//! `--features smt` opt-in was removed.

pub mod engine {
    use super::super::annotation::{
        DiscoveredValue, DiscoveredValues, SignalAbstraction, SvAnnotation,
    };
    use super::super::ast::*;
    use super::super::kripke;
    use std::collections::{HashMap, HashSet};
    const MAX_VALUES_PER_SIGNAL: usize = 32;

    /// Discover significant register values by querying z3.
    pub fn discover_significant_values(
        module: &Module,
        annotation: &SvAnnotation,
    ) -> HashMap<String, DiscoveredValues> {
        let mut results = HashMap::new();

        // Check both signals and inputs for discover abstraction
        let discover_signals: Vec<&str> = annotation
            .signals
            .iter()
            .filter(|s| s.preserve && s.abstraction == SignalAbstraction::Discover)
            .map(|s| s.name.as_str())
            .chain(
                annotation
                    .inputs
                    .iter()
                    .filter(|i| i.preserve && i.abstraction == SignalAbstraction::Discover)
                    .map(|i| i.name.as_str()),
            )
            .collect();

        if discover_signals.is_empty() {
            return results;
        }

        let mut comb_defs = collect_comb_definitions(module);
        let guards = collect_guard_exprs(module);
        let widths = collect_signal_widths(module);

        // Inject localparam/parameter values as combinational definitions.
        // Without this, the SMT engine treats parameter names as free variables,
        // producing spurious solutions for arbitrary parameter values.
        for param in &module.parameters {
            comb_defs
                .entry(param.name.clone())
                .or_insert(Expr::Number(param.default_value));
        }
        // Sidecar parameter overrides take precedence
        for (name, value) in &annotation.parameters {
            comb_defs.insert(name.clone(), Expr::Number(*value));
        }

        for signal_name in &discover_signals {
            let width = widths.get(*signal_name).copied().unwrap_or(32);
            let relevant_guards: Vec<&GuardExpr> = guards
                .iter()
                .filter(|g| {
                    let deps = collect_expr_deps(&g.expr, &comb_defs);
                    deps.contains(*signal_name)
                })
                .collect();

            if relevant_guards.is_empty() {
                continue;
            }

            let mut all_values: Vec<(i64, String)> = Vec::new();

            // Run z3 queries inside a scoped context
            let guard_exprs: Vec<(Expr, usize)> = relevant_guards
                .iter()
                .map(|g| (g.expr.clone(), g.line))
                .collect();
            let comb_defs_clone = comb_defs.clone();
            let widths_clone = widths.clone();
            let sig_name = signal_name.to_string();

            let cfg = z3::Config::new();
            let smt_values: Vec<(i64, String)> = z3::with_z3_config(&cfg, move || {
                let mut found = Vec::new();
                for (guard_expr, line) in &guard_exprs {
                    let solver = z3::Solver::new();
                    let mut variables: HashMap<String, z3::ast::BV> = HashMap::new();

                    let formula = expr_to_z3(
                        guard_expr,
                        &mut variables,
                        &comb_defs_clone,
                        &widths_clone,
                        width,
                    );

                    let zero = z3::ast::BV::from_i64(0, formula.get_size());
                    solver.assert(formula.eq(&zero).not());

                    let target_var = match variables.get(sig_name.as_str()) {
                        Some(v) => v.clone(),
                        None => continue,
                    };

                    let values = enumerate_values(&solver, &target_var, MAX_VALUES_PER_SIGNAL);
                    for val in values {
                        if !found.iter().any(|(v, _): &(i64, String)| *v == val) {
                            let provenance = format!(
                                "SMT: guard ({}) at line {}",
                                expr_to_short_string(guard_expr),
                                line
                            );
                            found.push((val, provenance));
                        }
                    }
                }
                found
            });

            all_values.extend(smt_values);

            // Merge syntactically-found constants
            let syntactic = kripke::scan_significant_constants(module);
            if let Some(syn_vals) = syntactic.get(*signal_name) {
                for &val in syn_vals {
                    if !all_values.iter().any(|(v, _)| *v == val) {
                        all_values.push((val, "syntactic: direct constant".to_string()));
                    }
                }
            }

            if !all_values.is_empty() {
                all_values.sort_by_key(|(v, _)| *v);
                let discovered = DiscoveredValues {
                    values: all_values
                        .iter()
                        .map(|(val, from)| DiscoveredValue {
                            value: *val,
                            name: format!("VAL_{val}"),
                            from: Some(from.clone()),
                        })
                        .collect(),
                    catch_all: "OTHER".to_string(),
                };
                results.insert(signal_name.to_string(), discovered);
            }
        }

        results
    }

    /// Discover significant values for signals shared across module boundaries.
    ///
    /// For each connection with `abstraction: "discover"`, this function:
    /// 1. Collects guard expressions from ALL modules that reference the signal
    /// 2. Runs SMT discovery on the combined guard set
    /// 3. Returns results keyed by `"module.port"` (matching the multi-module sidecar format)
    ///
    /// This ensures both sides of a connection use the same discovered domain.
    pub fn discover_cross_module_values(
        modules: &[(&Module, &str)], // (parsed module, module name)
        connections: &[super::super::annotation::ConnectionSpec],
        parameters: &HashMap<String, HashMap<String, i64>>, // module_name -> param overrides
    ) -> HashMap<String, DiscoveredValues> {
        use super::super::annotation::SignalAbstraction;
        let mut results = HashMap::new();

        for conn in connections {
            if conn.abstraction != SignalAbstraction::Discover {
                continue;
            }

            let (from_mod, from_port) = match conn.parse_from() {
                Some(v) => v,
                None => continue,
            };
            let (to_mod, to_port) = match conn.parse_to() {
                Some(v) => v,
                None => continue,
            };

            // Collect guards from all modules that reference this signal
            let mut all_guards: Vec<GuardExpr> = Vec::new();
            let mut combined_comb_defs: HashMap<String, Expr> = HashMap::new();
            let mut combined_widths: HashMap<String, u32> = HashMap::new();
            let mut target_width: u32 = 32;

            for (module, mod_name) in modules {
                // Determine the local signal name for this module
                let local_name = if *mod_name == from_mod {
                    from_port
                } else if *mod_name == to_mod {
                    to_port
                } else {
                    continue;
                };

                let mut comb_defs = collect_comb_definitions(module);
                let guards = collect_guard_exprs(module);
                let widths = collect_signal_widths(module);

                // Inject parameters
                for param in &module.parameters {
                    comb_defs
                        .entry(param.name.clone())
                        .or_insert(Expr::Number(param.default_value));
                }
                if let Some(params) = parameters.get(*mod_name) {
                    for (name, value) in params {
                        comb_defs.insert(name.clone(), Expr::Number(*value));
                    }
                }

                // Find guards that reference the local signal name
                for guard in guards {
                    let deps = collect_expr_deps(&guard.expr, &comb_defs);
                    if deps.contains(local_name) {
                        // Rename the local signal to the canonical connection name
                        let renamed_expr =
                            rename_signal_in_expr(&guard.expr, local_name, from_port);
                        all_guards.push(GuardExpr {
                            expr: renamed_expr,
                            line: guard.line,
                        });
                    }
                }

                if let Some(w) = widths.get(local_name) {
                    target_width = *w;
                }
                // Merge comb defs (rename local signal → canonical name)
                for (k, v) in comb_defs {
                    if k == local_name && *mod_name != from_mod {
                        combined_comb_defs.insert(from_port.to_string(), v);
                    } else {
                        combined_comb_defs.entry(k).or_insert(v);
                    }
                }
                for (k, v) in widths {
                    combined_widths.entry(k).or_insert(v);
                }
            }

            if all_guards.is_empty() {
                continue;
            }

            // Run SMT discovery on the combined guard set
            let guard_exprs: Vec<(Expr, usize)> = all_guards
                .iter()
                .map(|g| (g.expr.clone(), g.line))
                .collect();
            let sig_name = from_port.to_string();
            let width = target_width;
            let comb_defs_for_z3 = combined_comb_defs.clone();
            let widths_for_z3 = combined_widths.clone();

            let cfg = z3::Config::new();
            let smt_values: Vec<(i64, String)> = z3::with_z3_config(&cfg, move || {
                let mut found = Vec::new();
                for (guard_expr, line) in &guard_exprs {
                    let solver = z3::Solver::new();
                    let mut variables: HashMap<String, z3::ast::BV> = HashMap::new();

                    let formula = expr_to_z3(
                        guard_expr,
                        &mut variables,
                        &comb_defs_for_z3,
                        &widths_for_z3,
                        width,
                    );

                    let zero = z3::ast::BV::from_i64(0, formula.get_size());
                    solver.assert(formula.eq(&zero).not());

                    let target_var = match variables.get(sig_name.as_str()) {
                        Some(v) => v.clone(),
                        None => continue,
                    };

                    let values = enumerate_values(&solver, &target_var, MAX_VALUES_PER_SIGNAL);
                    for val in values {
                        if !found.iter().any(|(v, _): &(i64, String)| *v == val) {
                            let provenance = format!(
                                "SMT: cross-module guard ({}) at line {}",
                                expr_to_short_string(guard_expr),
                                line
                            );
                            found.push((val, provenance));
                        }
                    }
                }
                found
            });

            if !smt_values.is_empty() {
                let mut all_values = smt_values;
                all_values.sort_by_key(|(v, _)| *v);
                let discovered = DiscoveredValues {
                    values: all_values
                        .iter()
                        .map(|(val, from)| DiscoveredValue {
                            value: *val,
                            name: format!("VAL_{val}"),
                            from: Some(from.clone()),
                        })
                        .collect(),
                    catch_all: "OTHER".to_string(),
                };
                let key = format!("{from_mod}.{from_port}");
                results.insert(key, discovered);
            }
        }

        results
    }

    /// Rename a signal identifier in an expression tree.
    fn rename_signal_in_expr(expr: &Expr, old_name: &str, new_name: &str) -> Expr {
        match expr {
            Expr::Ident(name) => {
                if name == old_name {
                    Expr::Ident(new_name.to_string())
                } else {
                    expr.clone()
                }
            }
            Expr::BinOp { op, left, right } => Expr::BinOp {
                op: *op,
                left: Box::new(rename_signal_in_expr(left, old_name, new_name)),
                right: Box::new(rename_signal_in_expr(right, old_name, new_name)),
            },
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
            } => Expr::Ternary {
                cond: Box::new(rename_signal_in_expr(cond, old_name, new_name)),
                then_expr: Box::new(rename_signal_in_expr(then_expr, old_name, new_name)),
                else_expr: Box::new(rename_signal_in_expr(else_expr, old_name, new_name)),
            },
            Expr::Not(inner) => {
                Expr::Not(Box::new(rename_signal_in_expr(inner, old_name, new_name)))
            }
            Expr::BitSelect { base, index } => Expr::BitSelect {
                base: Box::new(rename_signal_in_expr(base, old_name, new_name)),
                index: Box::new(rename_signal_in_expr(index, old_name, new_name)),
            },
            Expr::BitSlice { base, msb, lsb } => Expr::BitSlice {
                base: Box::new(rename_signal_in_expr(base, old_name, new_name)),
                msb: Box::new(rename_signal_in_expr(msb, old_name, new_name)),
                lsb: Box::new(rename_signal_in_expr(lsb, old_name, new_name)),
            },
            Expr::Concat(parts) => Expr::Concat(
                parts
                    .iter()
                    .map(|p| rename_signal_in_expr(p, old_name, new_name))
                    .collect(),
            ),
            _ => expr.clone(),
        }
    }

    // -----------------------------------------------------------------------
    // Guard expression collection
    // -----------------------------------------------------------------------

    struct GuardExpr {
        expr: Expr,
        line: usize,
    }

    fn collect_guard_exprs(module: &Module) -> Vec<GuardExpr> {
        let mut guards = Vec::new();
        for block in &module.always_blocks {
            match block {
                AlwaysBlock::AlwaysFF { body, .. } => {
                    collect_guards_from_stmt(body, &mut guards, 0);
                }
                AlwaysBlock::AlwaysComb { body } => {
                    collect_guards_from_stmt(body, &mut guards, 0);
                }
            }
        }
        guards
    }

    fn collect_guards_from_stmt(stmt: &Statement, guards: &mut Vec<GuardExpr>, line: usize) {
        match stmt {
            Statement::If {
                cond,
                then_branch,
                else_branch,
            } => {
                guards.push(GuardExpr {
                    expr: cond.clone(),
                    line,
                });
                collect_guards_from_stmt(then_branch, guards, line);
                if let Some(e) = else_branch {
                    collect_guards_from_stmt(e, guards, line);
                }
            }
            Statement::Case {
                selector,
                branches,
                default,
                ..
            } => {
                for branch in branches {
                    if let Ok(n) = branch.label.parse::<i64>() {
                        guards.push(GuardExpr {
                            expr: Expr::BinOp {
                                op: BinOp::Eq,
                                left: Box::new(Expr::Ident(selector.clone())),
                                right: Box::new(Expr::Number(n)),
                            },
                            line,
                        });
                    }
                    collect_guards_from_stmt(&branch.body, guards, line);
                }
                if let Some(d) = default {
                    collect_guards_from_stmt(d, guards, line);
                }
            }
            Statement::Block(stmts) => {
                for s in stmts {
                    collect_guards_from_stmt(s, guards, line);
                }
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Combinational definitions & signal widths
    // -----------------------------------------------------------------------

    fn collect_comb_definitions(module: &Module) -> HashMap<String, Expr> {
        let mut defs = HashMap::new();
        for a in &module.assigns {
            defs.insert(a.target.name().to_string(), a.value.clone());
        }
        for block in &module.always_blocks {
            if let AlwaysBlock::AlwaysComb { body } = block {
                collect_comb_defs_from_stmt(body, &mut defs);
            }
        }
        defs
    }

    fn collect_comb_defs_from_stmt(stmt: &Statement, defs: &mut HashMap<String, Expr>) {
        match stmt {
            Statement::BlockingAssign { target, value } => {
                defs.insert(target.name().to_string(), value.clone());
            }
            Statement::Block(stmts) => {
                for s in stmts {
                    collect_comb_defs_from_stmt(s, defs);
                }
            }
            _ => {}
        }
    }

    fn collect_signal_widths(module: &Module) -> HashMap<String, u32> {
        let mut widths = HashMap::new();
        for decl in &module.declarations {
            match decl {
                Declaration::Logic { name, width } => {
                    widths.insert(name.clone(), *width as u32);
                }
                Declaration::Enum {
                    var_name: Some(var),
                    variants,
                    ..
                } => {
                    let bits = (variants.len() as f64).log2().ceil().max(1.0) as u32;
                    widths.insert(var.clone(), bits);
                }
                _ => {}
            }
        }
        for port in &module.ports {
            widths.insert(port.name.clone(), port.width as u32);
        }
        widths
    }

    // -----------------------------------------------------------------------
    // Expression dependency analysis
    // -----------------------------------------------------------------------

    fn collect_expr_deps(expr: &Expr, comb_defs: &HashMap<String, Expr>) -> HashSet<String> {
        let mut deps = HashSet::new();
        collect_expr_deps_inner(expr, comb_defs, &mut deps, &mut HashSet::new());
        deps
    }

    fn collect_expr_deps_inner(
        expr: &Expr,
        comb_defs: &HashMap<String, Expr>,
        deps: &mut HashSet<String>,
        visited: &mut HashSet<String>,
    ) {
        match expr {
            Expr::Ident(name) => {
                if visited.contains(name) {
                    return;
                }
                visited.insert(name.clone());
                deps.insert(name.clone());
                if let Some(def) = comb_defs.get(name) {
                    collect_expr_deps_inner(def, comb_defs, deps, visited);
                }
            }
            Expr::Number(_) | Expr::Bool(_) => {}
            Expr::Not(inner) => collect_expr_deps_inner(inner, comb_defs, deps, visited),
            Expr::BinOp { left, right, .. } => {
                collect_expr_deps_inner(left, comb_defs, deps, visited);
                collect_expr_deps_inner(right, comb_defs, deps, visited);
            }
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
            } => {
                collect_expr_deps_inner(cond, comb_defs, deps, visited);
                collect_expr_deps_inner(then_expr, comb_defs, deps, visited);
                collect_expr_deps_inner(else_expr, comb_defs, deps, visited);
            }
            Expr::BitSelect { base, index } => {
                collect_expr_deps_inner(base, comb_defs, deps, visited);
                collect_expr_deps_inner(index, comb_defs, deps, visited);
            }
            Expr::BitSlice { base, msb, lsb } => {
                collect_expr_deps_inner(base, comb_defs, deps, visited);
                collect_expr_deps_inner(msb, comb_defs, deps, visited);
                collect_expr_deps_inner(lsb, comb_defs, deps, visited);
            }
            Expr::Concat(parts) => {
                for p in parts {
                    collect_expr_deps_inner(p, comb_defs, deps, visited);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Expr → z3 BV translation (z3 v0.20 API: no context parameter)
    // -----------------------------------------------------------------------

    fn expr_to_z3(
        expr: &Expr,
        variables: &mut HashMap<String, z3::ast::BV>,
        comb_defs: &HashMap<String, Expr>,
        widths: &HashMap<String, u32>,
        default_width: u32,
    ) -> z3::ast::BV {
        match expr {
            Expr::Ident(name) => {
                if let Some(def) = comb_defs.get(name) {
                    return expr_to_z3(def, variables, comb_defs, widths, default_width);
                }
                if let Some(var) = variables.get(name) {
                    return var.clone();
                }
                let width = widths.get(name).copied().unwrap_or(default_width);
                let var = z3::ast::BV::new_const(name.as_str(), width);
                variables.insert(name.clone(), var.clone());
                var
            }
            Expr::Number(n) => z3::ast::BV::from_i64(*n, default_width),
            Expr::Bool(b) => z3::ast::BV::from_i64(*b as i64, default_width),
            Expr::Not(inner) => {
                let v = expr_to_z3(inner, variables, comb_defs, widths, default_width);
                v.bvnot()
            }
            Expr::BinOp { op, left, right } => {
                let l = expr_to_z3(left, variables, comb_defs, widths, default_width);
                let r = expr_to_z3(right, variables, comb_defs, widths, default_width);
                let (l, r) = match_widths(l, r);

                match op {
                    BinOp::Add => l.bvadd(&r),
                    BinOp::Sub => l.bvsub(&r),
                    BinOp::Mul => l.bvmul(&r),
                    BinOp::Div => l.bvsdiv(&r),
                    BinOp::Mod => l.bvsmod(&r),
                    BinOp::Shl => l.bvshl(&r),
                    BinOp::Shr => l.bvlshr(&r),
                    BinOp::BitAnd => l.bvand(&r),
                    BinOp::BitOr => l.bvor(&r),
                    BinOp::BitXor => l.bvxor(&r),
                    BinOp::And => {
                        let zero = z3::ast::BV::from_i64(0, l.get_size());
                        let one = z3::ast::BV::from_i64(1, l.get_size());
                        let l_nz = l.eq(&zero).not();
                        let r_nz = r.eq(&zero).not();
                        z3::ast::Bool::and(&[&l_nz, &r_nz]).ite(&one, &zero)
                    }
                    BinOp::Or => {
                        let zero = z3::ast::BV::from_i64(0, l.get_size());
                        let one = z3::ast::BV::from_i64(1, l.get_size());
                        let l_nz = l.eq(&zero).not();
                        let r_nz = r.eq(&zero).not();
                        z3::ast::Bool::or(&[&l_nz, &r_nz]).ite(&one, &zero)
                    }
                    BinOp::Eq => {
                        let one = z3::ast::BV::from_i64(1, l.get_size());
                        let zero = z3::ast::BV::from_i64(0, l.get_size());
                        l.eq(&r).ite(&one, &zero)
                    }
                    BinOp::Ne => {
                        let one = z3::ast::BV::from_i64(1, l.get_size());
                        let zero = z3::ast::BV::from_i64(0, l.get_size());
                        l.eq(&r).not().ite(&one, &zero)
                    }
                    BinOp::Lt => {
                        let one = z3::ast::BV::from_i64(1, l.get_size());
                        let zero = z3::ast::BV::from_i64(0, l.get_size());
                        l.bvslt(&r).ite(&one, &zero)
                    }
                    BinOp::Le => {
                        let one = z3::ast::BV::from_i64(1, l.get_size());
                        let zero = z3::ast::BV::from_i64(0, l.get_size());
                        l.bvsle(&r).ite(&one, &zero)
                    }
                    BinOp::Gt => {
                        let one = z3::ast::BV::from_i64(1, l.get_size());
                        let zero = z3::ast::BV::from_i64(0, l.get_size());
                        l.bvsgt(&r).ite(&one, &zero)
                    }
                    BinOp::Ge => {
                        let one = z3::ast::BV::from_i64(1, l.get_size());
                        let zero = z3::ast::BV::from_i64(0, l.get_size());
                        l.bvsge(&r).ite(&one, &zero)
                    }
                }
            }
            Expr::Ternary {
                cond,
                then_expr,
                else_expr,
            } => {
                let c = expr_to_z3(cond, variables, comb_defs, widths, default_width);
                let t = expr_to_z3(then_expr, variables, comb_defs, widths, default_width);
                let e = expr_to_z3(else_expr, variables, comb_defs, widths, default_width);
                let zero = z3::ast::BV::from_i64(0, c.get_size());
                let cond_bool = c.eq(&zero).not();
                let (t, e) = match_widths(t, e);
                cond_bool.ite(&t, &e)
            }
            Expr::BitSelect { base, index } => {
                let b = expr_to_z3(base, variables, comb_defs, widths, default_width);
                match index.as_ref() {
                    Expr::Number(i) => {
                        let i = *i as u32;
                        b.extract(i, i).zero_ext(default_width - 1)
                    }
                    _ => {
                        let idx = expr_to_z3(index, variables, comb_defs, widths, default_width);
                        let (b, idx) = match_widths(b, idx);
                        let shifted = b.bvlshr(&idx);
                        let one = z3::ast::BV::from_i64(1, shifted.get_size());
                        shifted.bvand(&one)
                    }
                }
            }
            Expr::BitSlice { base, msb, lsb } => {
                let b = expr_to_z3(base, variables, comb_defs, widths, default_width);
                match (msb.as_ref(), lsb.as_ref()) {
                    (Expr::Number(m), Expr::Number(l)) => {
                        let m = *m as u32;
                        let l = *l as u32;
                        let slice = b.extract(m, l);
                        let slice_width = m - l + 1;
                        if slice_width < default_width {
                            slice.zero_ext(default_width - slice_width)
                        } else {
                            slice
                        }
                    }
                    _ => b,
                }
            }
            Expr::Concat(parts) => {
                if parts.is_empty() {
                    return z3::ast::BV::from_i64(0, default_width);
                }
                let mut result = expr_to_z3(&parts[0], variables, comb_defs, widths, default_width);
                for part in &parts[1..] {
                    let p = expr_to_z3(part, variables, comb_defs, widths, default_width);
                    result = result.concat(&p);
                }
                let total = result.get_size();
                if total > default_width {
                    result.extract(default_width - 1, 0)
                } else if total < default_width {
                    result.zero_ext(default_width - total)
                } else {
                    result
                }
            }
        }
    }

    fn match_widths(a: z3::ast::BV, b: z3::ast::BV) -> (z3::ast::BV, z3::ast::BV) {
        let wa = a.get_size();
        let wb = b.get_size();
        if wa == wb {
            (a, b)
        } else if wa > wb {
            (a, b.zero_ext(wa - wb))
        } else {
            (a.zero_ext(wb - wa), b)
        }
    }

    // -----------------------------------------------------------------------
    // Value enumeration via blocking clauses
    // -----------------------------------------------------------------------

    fn enumerate_values(solver: &z3::Solver, target_var: &z3::ast::BV, max: usize) -> Vec<i64> {
        let mut values = Vec::new();
        loop {
            if values.len() >= max {
                break;
            }
            match solver.check() {
                z3::SatResult::Sat => {
                    let model = solver.get_model().unwrap();
                    if let Some(val) = model.eval(target_var, true) {
                        if let Some(n) = val.as_i64() {
                            values.push(n);
                            let blocked = z3::ast::BV::from_i64(n, target_var.get_size());
                            solver.assert(target_var.eq(&blocked).not());
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        values.sort();
        values
    }

    fn expr_to_short_string(expr: &Expr) -> String {
        match expr {
            Expr::Ident(s) => s.clone(),
            Expr::Number(n) => n.to_string(),
            Expr::Bool(b) => b.to_string(),
            Expr::Not(inner) => format!("!{}", expr_to_short_string(inner)),
            Expr::BinOp { op, left, right } => {
                let op_str = match op {
                    BinOp::Eq => "==",
                    BinOp::Ne => "!=",
                    BinOp::Lt => "<",
                    BinOp::Le => "<=",
                    BinOp::Gt => ">",
                    BinOp::Ge => ">=",
                    BinOp::Add => "+",
                    BinOp::Sub => "-",
                    BinOp::Mul => "*",
                    BinOp::Div => "/",
                    BinOp::Mod => "%",
                    BinOp::And => "&&",
                    BinOp::Or => "||",
                    BinOp::BitAnd => "&",
                    BinOp::BitOr => "|",
                    BinOp::BitXor => "^",
                    BinOp::Shl => "<<",
                    BinOp::Shr => ">>",
                };
                format!(
                    "{} {} {}",
                    expr_to_short_string(left),
                    op_str,
                    expr_to_short_string(right)
                )
            }
            _ => "...".to_string(),
        }
    }
}

pub use engine::discover_significant_values;

#[cfg(test)]
mod tests {
    use super::engine::*;
    use crate::adapter::systemverilog::annotation::*;
    use crate::adapter::systemverilog::parser;

    #[test]
    fn discover_direct_constant() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst);
                logic [7:0] cmd;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) cmd <= 0;
                    else if (cmd == 42) cmd <= 0;
                end
            endmodule"#,
        )
        .unwrap();

        let ann: SvAnnotation = serde_json::from_str(
            r#"{
            "module": "test",
            "signals": [{"name": "cmd", "abstraction": "discover"}],
            "properties": []
        }"#,
        )
        .unwrap();

        let results = discover_significant_values(&module, &ann);
        let cmd_vals = results
            .get("cmd")
            .expect("cmd should have discovered values");
        assert!(cmd_vals.values.iter().any(|v| v.value == 42));
    }

    #[test]
    fn discover_through_combinational_logic() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst);
                logic [7:0] x;
                logic [7:0] y;
                assign y = x * 4;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) x <= 0;
                    else if (y == 12) x <= 0;
                end
            endmodule"#,
        )
        .unwrap();

        let ann: SvAnnotation = serde_json::from_str(
            r#"{
            "module": "test",
            "signals": [{"name": "x", "abstraction": "discover"}],
            "properties": []
        }"#,
        )
        .unwrap();

        let results = discover_significant_values(&module, &ann);
        let x_vals = results.get("x").expect("x should have discovered values");
        assert!(
            x_vals.values.iter().any(|v| v.value == 3),
            "SMT should discover x=3 from y=x*4; y==12. Found: {:?}",
            x_vals.values
        );
    }

    #[test]
    fn discover_through_shift() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst);
                logic [7:0] x;
                logic [7:0] y;
                assign y = x << 2;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) x <= 0;
                    else if (y == 16) x <= 0;
                end
            endmodule"#,
        )
        .unwrap();

        let ann: SvAnnotation = serde_json::from_str(
            r#"{
            "module": "test",
            "signals": [{"name": "x", "abstraction": "discover"}],
            "properties": []
        }"#,
        )
        .unwrap();

        let results = discover_significant_values(&module, &ann);
        let x_vals = results.get("x").expect("x should have discovered values");
        assert!(
            x_vals.values.iter().any(|v| v.value == 4),
            "SMT should discover x=4 from y=x<<2; y==16. Found: {:?}",
            x_vals.values
        );
    }

    #[test]
    fn discover_from_case_labels() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst);
                logic [7:0] cmd;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) cmd <= 0;
                    else case (cmd)
                        0: cmd <= 1;
                        3: cmd <= 0;
                        255: cmd <= 0;
                        default: ;
                    endcase
                end
            endmodule"#,
        )
        .unwrap();

        let ann: SvAnnotation = serde_json::from_str(
            r#"{
            "module": "test",
            "signals": [{"name": "cmd", "abstraction": "discover"}],
            "properties": []
        }"#,
        )
        .unwrap();

        let results = discover_significant_values(&module, &ann);
        let cmd_vals = results
            .get("cmd")
            .expect("cmd should have discovered values");
        assert!(cmd_vals.values.iter().any(|v| v.value == 0));
        assert!(cmd_vals.values.iter().any(|v| v.value == 3));
        // 255 as i64 for 8-bit: could be -1 in signed BV. Check both.
        assert!(
            cmd_vals
                .values
                .iter()
                .any(|v| v.value == 255 || v.value == -1),
            "Should find 255 (or -1 in signed 8-bit). Found: {:?}",
            cmd_vals.values
        );
    }

    #[test]
    fn no_discover_for_non_discover_signals() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst);
                logic [7:0] cmd;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) cmd <= 0;
                    else if (cmd == 42) cmd <= 0;
                end
            endmodule"#,
        )
        .unwrap();

        let ann: SvAnnotation = serde_json::from_str(
            r#"{
            "module": "test",
            "signals": [{"name": "cmd", "abstraction": "bounded_counter", "bound": 7}],
            "properties": []
        }"#,
        )
        .unwrap();

        let results = discover_significant_values(&module, &ann);
        assert!(results.is_empty());
    }
}
