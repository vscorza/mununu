//! AIGER ASCII (.aag) format parser.
//!
//! Parses the AAG (ASCII AIGER) format as defined in the AIGER 1.9 specification.
//! Binary .aig format is not yet supported.

use super::ast::*;
use crate::adapter::{AdapterError, AdapterErrorKind, SourceLocation};

/// Parse an AAG (ASCII AIGER) file.
pub fn parse(content: &str) -> Result<Circuit, AdapterError> {
    let mut lines = content.lines().enumerate();

    // Parse header: aag M I L O A [B [C [J [F]]]]
    let (line_num, header_line) = lines
        .next()
        .ok_or_else(|| err(0, "empty file (expected 'aag' header line)"))?;

    let parts: Vec<&str> = header_line.split_whitespace().collect();
    if parts.is_empty() || parts[0] != "aag" {
        return Err(err(
            line_num,
            &format!(
                "expected 'aag' header but found '{}' (binary .aig format is not supported)",
                parts[0]
            ),
        ));
    }
    if parts.len() < 6 {
        return Err(err(
            line_num,
            &format!(
                "incomplete header: expected 'aag M I L O A [B [C [J [F]]]]' but found {} field(s)",
                parts.len()
            ),
        ));
    }

    let max_var = parse_usize(parts[1], line_num, "M")?;
    let num_inputs = parse_usize(parts[2], line_num, "I")?;
    let num_latches = parse_usize(parts[3], line_num, "L")?;
    let num_outputs = parse_usize(parts[4], line_num, "O")?;
    let num_gates = parse_usize(parts[5], line_num, "A")?;
    let num_bad = if parts.len() > 6 {
        parse_usize(parts[6], line_num, "B")?
    } else {
        0
    };
    let num_constraints = if parts.len() > 7 {
        parse_usize(parts[7], line_num, "C")?
    } else {
        0
    };
    let num_justice = if parts.len() > 8 {
        parse_usize(parts[8], line_num, "justice count (J)")?
    } else {
        0
    };
    let num_fairness = if parts.len() > 9 {
        parse_usize(parts[9], line_num, "fairness count (F)")?
    } else {
        0
    };

    // Parse inputs (one literal per line)
    let mut inputs = Vec::with_capacity(num_inputs);
    for _ in 0..num_inputs {
        let (ln, line) = next_content_line(&mut lines)?;
        let lit = parse_usize(line.trim(), ln, "input literal")?;
        inputs.push(lit);
    }

    // Parse latches (current next [init])
    let mut latches = Vec::with_capacity(num_latches);
    for _ in 0..num_latches {
        let (ln, line) = next_content_line(&mut lines)?;
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            return Err(err(
                ln,
                &format!(
                    "latch definition requires at least 2 fields (current next [init]) but found {}",
                    parts.len()
                ),
            ));
        }
        let current = parse_usize(parts[0], ln, "latch current")?;
        let next = parse_usize(parts[1], ln, "latch next")?;
        let init = if parts.len() > 2 {
            parse_usize(parts[2], ln, "latch init")? as u8
        } else {
            0
        };
        latches.push(Latch {
            current,
            next,
            init,
        });
    }

    // Parse outputs (one literal per line) -- skip, we use bad outputs instead
    let mut _outputs = Vec::with_capacity(num_outputs);
    for _ in 0..num_outputs {
        let (ln, line) = next_content_line(&mut lines)?;
        let _lit = parse_usize(line.trim(), ln, "output literal")?;
        _outputs.push(_lit);
    }

    // Parse bad outputs
    let mut bad_outputs = Vec::with_capacity(num_bad);
    for _ in 0..num_bad {
        let (ln, line) = next_content_line(&mut lines)?;
        let lit = parse_usize(line.trim(), ln, "bad literal")?;
        bad_outputs.push(lit);
    }

    // Parse constraints
    let mut constraints = Vec::with_capacity(num_constraints);
    for _ in 0..num_constraints {
        let (ln, line) = next_content_line(&mut lines)?;
        let lit = parse_usize(line.trim(), ln, "constraint literal")?;
        constraints.push(lit);
    }

    // Parse justice sets
    let mut justice_sets = Vec::with_capacity(num_justice);
    for _ in 0..num_justice {
        let (ln, line) = next_content_line(&mut lines)?;
        let size = parse_usize(line.trim(), ln, "justice size")?;
        let mut set = Vec::with_capacity(size);
        for _ in 0..size {
            let (ln2, line2) = next_content_line(&mut lines)?;
            let lit = parse_usize(line2.trim(), ln2, "justice literal")?;
            set.push(lit);
        }
        justice_sets.push(set);
    }

    // Parse fairness constraints (one literal per line)
    let mut fairness = Vec::with_capacity(num_fairness);
    for _ in 0..num_fairness {
        let (ln, line) = next_content_line(&mut lines)?;
        let lit = parse_usize(line.trim(), ln, "fairness literal")?;
        fairness.push(lit);
    }

    // Parse AND gates (lhs rhs0 rhs1)
    let mut gates = Vec::with_capacity(num_gates);
    for _ in 0..num_gates {
        let (ln, line) = next_content_line(&mut lines)?;
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            return Err(err(
                ln,
                &format!(
                    "AND gate requires 3 fields (lhs rhs0 rhs1) but found {}",
                    parts.len()
                ),
            ));
        }
        let lhs = parse_usize(parts[0], ln, "gate lhs")?;
        let rhs0 = parse_usize(parts[1], ln, "gate rhs0")?;
        let rhs1 = parse_usize(parts[2], ln, "gate rhs1")?;
        gates.push(Gate { lhs, rhs0, rhs1 });
    }

    // Parse symbol table and comments
    let mut symbols = SymbolTable {
        input_names: vec![None; num_inputs],
        latch_names: vec![None; num_latches],
        output_names: vec![None; num_outputs],
        bad_names: vec![None; num_bad],
        constraint_names: vec![None; num_constraints],
    };

    for (_ln, line) in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('c') {
            break; // comment section or end
        }
        // Symbol: i<idx> name, l<idx> name, o<idx> name, b<idx> name
        if let Some(rest) = line.strip_prefix('i')
            && let Some((idx_str, name)) = rest.split_once(' ')
            && let Ok(idx) = idx_str.parse::<usize>()
            && idx < symbols.input_names.len()
        {
            symbols.input_names[idx] = Some(name.to_string());
        } else if let Some(rest) = line.strip_prefix('l')
            && let Some((idx_str, name)) = rest.split_once(' ')
            && let Ok(idx) = idx_str.parse::<usize>()
            && idx < symbols.latch_names.len()
        {
            symbols.latch_names[idx] = Some(name.to_string());
        } else if let Some(rest) = line.strip_prefix('b')
            && let Some((idx_str, name)) = rest.split_once(' ')
            && let Ok(idx) = idx_str.parse::<usize>()
            && idx < symbols.bad_names.len()
        {
            symbols.bad_names[idx] = Some(name.to_string());
        }
    }

    Ok(Circuit {
        max_var,
        inputs,
        latches,
        gates,
        bad_outputs,
        constraints,
        justice_sets,
        fairness,
        symbols,
    })
}

fn next_content_line<'a>(
    lines: &mut impl Iterator<Item = (usize, &'a str)>,
) -> Result<(usize, &'a str), AdapterError> {
    lines.next().ok_or_else(|| {
        err(
            0,
            "unexpected end of file (more lines expected per header counts)",
        )
    })
}

fn parse_usize(s: &str, line: usize, context: &str) -> Result<usize, AdapterError> {
    s.parse().map_err(|_| {
        err(
            line,
            &format!("invalid {context} '{s}' (expected unsigned integer)"),
        )
    })
}

/// Create a parse error at the given line.
///
/// Column is always 1 because the AIGER format is line-oriented;
/// each line contains a single declaration or definition.
fn err(line: usize, msg: &str) -> AdapterError {
    AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: msg.to_string(),
        location: Some(SourceLocation {
            line: line + 1,
            column: 1,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_alarm_circuit() {
        // Alarm circuit from the adapter spec
        let input = "\
aag 3 1 1 0 1 1
2
4 7
4
6 3 5
i0 sensor
l0 alarm
b0 alarm_on
";
        let circuit = parse(input).unwrap();
        assert_eq!(circuit.num_inputs(), 1);
        assert_eq!(circuit.num_latches(), 1);
        assert_eq!(circuit.gates.len(), 1);
        assert_eq!(circuit.bad_outputs.len(), 1);
        assert_eq!(circuit.bad_outputs[0], 4); // alarm literal
        assert_eq!(circuit.input_name(0), "sensor");
        assert_eq!(circuit.latch_name(0), "alarm");
    }

    #[test]
    fn eval_alarm_circuit() {
        let input = "\
aag 3 1 1 0 1 1
2
4 7
4
6 3 5
i0 sensor
l0 alarm
b0 alarm_on
";
        let circuit = parse(input).unwrap();

        // State: alarm=0, Input: sensor=0 → next_alarm = OR(0,0) = 0
        let mut values = vec![false, false, false]; // [unused, sensor=0, alarm=0]
        circuit.eval_gates(&mut values);
        let next = circuit.next_state(&values);
        assert_eq!(next, vec![false]); // alarm stays 0

        // State: alarm=0, Input: sensor=1 → next_alarm = OR(1,0) = 1
        let mut values = vec![false, true, false]; // [unused, sensor=1, alarm=0]
        circuit.eval_gates(&mut values);
        let next = circuit.next_state(&values);
        assert_eq!(next, vec![true]); // alarm turns on

        // State: alarm=1, Input: sensor=0 → next_alarm = OR(0,1) = 1
        let mut values = vec![false, false, true]; // [unused, sensor=0, alarm=1]
        circuit.eval_gates(&mut values);
        let next = circuit.next_state(&values);
        assert_eq!(next, vec![true]); // alarm stays on (latched)
    }
}
