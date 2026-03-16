use predicates::prelude::*;
use std::error::Error;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use tempfile::tempdir;

fn write_simple_context(dir: &Path) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let context_path = dir.join("simple.ctxdsl");
    let sidecar_path = dir.join("simple_props.ctxdsl");
    let context_src = r"context simple {
    alphabet {
        label a;
    }
    automata {
        automaton Simple {
            states {
                state S0 initial;
                state S1;
            }
            transitions {
                transition S0 -> S1 on label a;
            }
        }
    }
}
";
    let sidecar_src = r"context simple_props {
    mu_formulas {
        formula reachability { over Simple; body = true; }
    }
}
";
    fs::write(&context_path, context_src)?;
    fs::write(&sidecar_path, sidecar_src)?;
    Ok((context_path, sidecar_path))
}

#[test]
fn context_merge_copies_outputs() -> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    let (context_path, sidecar_path) = write_simple_context(temp.path())?;
    let output_dir = temp.path().join("merged");

    assert_cmd::cargo::cargo_bin_cmd!("mununu")
        .current_dir(temp.path())
        .args([
            "context",
            "merge",
            context_path.to_str().unwrap(),
            sidecar_path.to_str().unwrap(),
            "--output",
            output_dir.to_str().unwrap(),
            "--force",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Merged context 'simple'"))
        .stdout(predicate::str::contains("Copied 2 file(s)"));

    assert!(output_dir.join("simple.ctxdsl").exists());
    assert!(output_dir.join("simple_props.ctxdsl").exists());
    Ok(())
}

#[test]
fn context_predicates_reports_results() -> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    let (context_path, sidecar_path) = write_simple_context(temp.path())?;

    assert_cmd::cargo::cargo_bin_cmd!("mununu")
        .current_dir(temp.path())
        .args([
            "context",
            "predicates",
            context_path.to_str().unwrap(),
            "--sidecar",
            sidecar_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Automaton: Simple"))
        .stdout(predicate::str::contains("has_enabled_transition"))
        .stdout(predicate::str::contains("is_deadlock_state"));
    Ok(())
}

#[test]
fn context_summarize_with_print_structure_outputs_to_stdout() -> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    let (context_path, sidecar_path) = write_simple_context(temp.path())?;

    let assert = assert_cmd::cargo::cargo_bin_cmd!("mununu")
        .current_dir(temp.path())
        .args([
            "context",
            "summarize",
            context_path.to_str().unwrap(),
            "--sidecar",
            sidecar_path.to_str().unwrap(),
            "--print-structure",
        ])
        .assert()
        .success();

    let output = String::from_utf8(assert.get_output().stdout.clone())?;
    assert!(output.contains("\"context\""));
    assert!(output.contains("Context Structure:"));
    assert!(output.contains("Automaton: Simple"));
    assert!(output.contains("States: 2"));

    Ok(())
}

#[test]
fn context_summarize_with_print_structure_outputs_to_file() -> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    let (context_path, sidecar_path) = write_simple_context(temp.path())?;
    let structure_file = temp.path().join("structure.txt");

    let assert = assert_cmd::cargo::cargo_bin_cmd!("mununu")
        .current_dir(temp.path())
        .args([
            "context",
            "summarize",
            context_path.to_str().unwrap(),
            "--sidecar",
            sidecar_path.to_str().unwrap(),
            "--print-structure",
            structure_file.to_str().unwrap(),
        ])
        .assert()
        .success();

    assert!(structure_file.exists());
    let structure_content = fs::read_to_string(&structure_file)?;
    assert!(structure_content.contains("Context Structure:"));
    assert!(structure_content.contains("Automaton: Simple"));
    assert!(structure_content.contains("States: 2"));

    let output = String::from_utf8(assert.get_output().stdout.clone())?;
    assert!(output.contains("\"context\""));
    assert!(output.contains("Context structure written to"));

    Ok(())
}

#[test]
fn context_eval_with_print_structure_outputs_structure() -> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    let (context_path, sidecar_path) = write_simple_context(temp.path())?;

    let assert = assert_cmd::cargo::cargo_bin_cmd!("mununu")
        .current_dir(temp.path())
        .args([
            "context",
            "eval",
            context_path.to_str().unwrap(),
            "--sidecar",
            sidecar_path.to_str().unwrap(),
            "--formula",
            "reachability",
            "--automaton",
            "Simple",
            "--print-structure",
        ])
        .assert()
        .success();

    let output = String::from_utf8(assert.get_output().stdout.clone())?;
    assert!(output.contains("Context Structure:"));
    assert!(output.contains("Automaton: Simple"));
    assert!(output.contains("Formula 'reachability'"));

    Ok(())
}

#[test]
fn context_eval_reports_satisfying_states() -> Result<(), Box<dyn Error>> {
    let temp = tempdir()?;
    let (context_path, sidecar_path) = write_simple_context(temp.path())?;

    assert_cmd::cargo::cargo_bin_cmd!("mununu")
        .current_dir(temp.path())
        .args([
            "context",
            "eval",
            context_path.to_str().unwrap(),
            "--sidecar",
            sidecar_path.to_str().unwrap(),
            "--formula",
            "reachability",
            "--automaton",
            "Simple",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("States satisfying: 2/2"))
        .stdout(predicate::str::contains("Initial states satisfying: 1/1"));
    Ok(())
}
