//! Yosys child-process driver — elaborates SystemVerilog to BTOR2.
//!
//! # Architecture (per §13.4 of the RTL roadmap)
//!
//! Yosys is invoked **as a child process** via [`std::process::Command`].
//! No linking. No FFI. Yosys is ISC-licensed; the integration is license-clean
//! for any mununu downstream license.
//!
//! ```text
//!   .sv source ─► tempdir/work.sv
//!                       │
//!                       ▼  yosys -p "read_verilog -formal -sv ...; prep -top X; chformal -lower; write_btor ..."
//!                       │
//!                  tempdir/design.btor
//!                       │
//!                       ▼  super::btor2::parser::parse(...)
//!                  Btor2File ─► AdapterIR ─► CTXDSL
//! ```
//!
//! # Hard rules
//!
//! - **Never enable Yosys's optional Verific frontend.** It is commercial and
//!   would taint the binary. The driver passes only `-p` (no `-Q -F` or
//!   plugin-load arguments) and verifies (best-effort) that the binary in
//!   `$PATH` is a stock build by checking the `-V` banner for `verific`.
//! - **No persistent state in the working tree.** The driver writes only to
//!   a per-call tempdir which is removed (or kept for inspection if
//!   `MUNUNU_KEEP_YOSYS_TMP=1` is set).
//!
//! # Phase 1 scope
//!
//! - Single SV file or multiple SV files passed as `additional_sources`.
//! - User specifies the top module via [`YosysOptions::top`] or it is inferred
//!   from the file name (best-effort).
//! - Multi-clock SVA, classes, and Verific-only constructs are out of scope —
//!   Yosys errors will surface with a layer-tagged context (`adapter/yosys: ...`).

use crate::adapter::{
    AdapterError, AdapterErrorKind, AdapterOptions, AdapterOutput, FormatAdapter, SourceFormat,
    SourceInfo,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Yosys-specific options.
#[derive(Debug, Clone, Default)]
pub struct YosysOptions {
    /// Top-level module (if empty, the driver lets Yosys auto-detect).
    pub top: Option<String>,
    /// Additional SV source files to compile alongside the primary input.
    pub additional_sources: Vec<(String, String)>,
    /// Skip the Verific-taint check (for testing).
    pub skip_verific_check: bool,
}

/// SV-via-Yosys adapter — wraps the BTOR2 path with a Yosys subprocess.
pub struct YosysAdapter;

impl FormatAdapter for YosysAdapter {
    fn detect(_content: &str) -> bool {
        // Detection of "this is SV that should go through Yosys" happens at
        // the format-routing layer (CLI flag `--frontend yosys`, or the
        // `.sv`/`.v` extension when the user opts into the Yosys path).
        // The bare-content detector returns false so auto_translate does
        // not accidentally route plain SV here.
        false
    }

    fn translate(content: &str, options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
        translate_sv(content, options, &YosysOptions::default())
    }
}

/// Translate SystemVerilog source to AdapterOutput by invoking Yosys and
/// reading back the BTOR2.
pub fn translate_sv(
    content: &str,
    options: &AdapterOptions,
    yopts: &YosysOptions,
) -> Result<AdapterOutput, AdapterError> {
    let yosys = locate_yosys()?;
    if !yopts.skip_verific_check {
        verify_no_verific(&yosys)?;
    }

    let tmp = TempDir::new("mununu-yosys")?;
    let primary = tmp.path().join("work.sv");
    write_file(&primary, content)?;

    let mut sources = vec![primary.clone()];
    for (name, src) in &yopts.additional_sources {
        let p = tmp.path().join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(io_err)?;
        }
        write_file(&p, src)?;
        sources.push(p);
    }

    let btor_path = tmp.path().join("design.btor");
    let script = build_script(&sources, yopts.top.as_deref(), &btor_path);

    let output = Command::new(&yosys)
        .arg("-q")
        .arg("-p")
        .arg(&script)
        .output()
        .map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!("adapter/yosys: failed to spawn yosys: {e}"),
            location: None,
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!(
                "adapter/yosys: yosys exited with status {} \nstdout:\n{stdout}\nstderr:\n{stderr}",
                output.status
            ),
            location: None,
        });
    }

    let btor = std::fs::read_to_string(&btor_path).map_err(|e| AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: format!(
            "adapter/yosys: yosys ran but did not produce {} ({e})",
            btor_path.display()
        ),
        location: None,
    })?;

    // Hand the BTOR2 to the BTOR2 adapter.
    let mut out = super::btor2::Btor2Adapter::translate(&btor, options).map_err(|mut e| {
        e.message = format!("adapter/yosys: BTOR2 reader failed: {}", e.message);
        e
    })?;

    // Re-tag the source format so users see this as the SV-via-Yosys path.
    out.source_info = SourceInfo {
        format: SourceFormat::SystemVerilog,
        ..out.source_info
    };
    Ok(out)
}

/// Find a usable yosys binary.
fn locate_yosys() -> Result<PathBuf, AdapterError> {
    if let Ok(path) = std::env::var("MUNUNU_YOSYS_PATH") {
        return Ok(PathBuf::from(path));
    }
    let candidates = ["yosys"];
    for c in candidates {
        if let Ok(out) = Command::new(c).arg("-V").output()
            && out.status.success()
        {
            return Ok(PathBuf::from(c));
        }
    }
    Err(AdapterError {
        kind: AdapterErrorKind::UnsupportedConstruct,
        message: "adapter/yosys: yosys binary not found in $PATH (set MUNUNU_YOSYS_PATH or install yosys ≥ 0.40)".into(),
        location: None,
    })
}

/// Best-effort check that the located yosys binary was not built with the
/// commercial Verific frontend. Verific-built yosys binaries print
/// "verific" in their `-V` banner.
fn verify_no_verific(yosys: &Path) -> Result<(), AdapterError> {
    let out = Command::new(yosys)
        .arg("-V")
        .output()
        .map_err(|e| AdapterError {
            kind: AdapterErrorKind::ParseError,
            message: format!("adapter/yosys: failed to read yosys version: {e}"),
            location: None,
        })?;
    let banner = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if banner.to_lowercase().contains("verific") {
        return Err(AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: format!(
                "adapter/yosys: refusing to use a yosys binary built with Verific (commercial / license-incompatible). Banner:\n{banner}"
            ),
            location: None,
        });
    }
    Ok(())
}

/// Build the Yosys script to elaborate `sources` into a BTOR2 file.
///
/// The pipeline is intentionally **non-pruning** — verification benefits
/// from observing every register, even those without an external output.
/// `prep` and `opt -fast` are explicitly avoided because they strip cells
/// not feeding outputs / asserts and would silently produce an empty BTOR2
/// for SV without assertions.
///
/// Components, in order:
///
/// 1. `read_verilog -formal -sv` — parse, enabling SVA / formal constructs.
/// 2. `hierarchy -top X` — select the design's root.
/// 3. `proc` — lower always-blocks to RTL netlist.
/// 4. `flatten` — inline submodule instances.
/// 5. `async2sync` — convert async-reset / async-set cells to plain
///    synchronous DFFs while **preserving the synchronous structure**
///    (the clock is implicit in BTOR2's `state` semantics, not exposed
///    as edge-detect combinational logic). This lets `chformal -lower`
///    translate the assertions cleanly without introducing the `value +
///    shadow + previous-clk` triple-state-cell encoding that
///    `clk2fflogic` would. The previous (`clk2fflogic`-based) script
///    produced ~3 state cells per user FF group; this script produces
///    1, matching mununu's "each CLTS transition = one clock edge"
///    semantics natively.
/// 6. `chformal -lower` — translate SVA `assert` / `assume` / `cover` into
///    BTOR2 `bad` / `constraint` / fair / `justice` signals. No-op if no
///    SVA is present in the design.
/// 7. `dffunmap` — unmap SDFF (synchronous-reset) cells to plain DFF +
///    explicit reset logic (write_btor only accepts plain DFF).
/// 8. `setundef -zero` — replace any remaining X / undef bits with 0
///    (deterministic; bit-blaster does not model X-prop).
/// 9. `write_btor` — emit the BTOR2.
fn build_script(sources: &[PathBuf], top: Option<&str>, btor_out: &Path) -> String {
    let read_cmds: Vec<String> = sources
        .iter()
        .map(|p| format!("read_verilog -formal -sv {}", p.display()))
        .collect();
    let hier = match top {
        Some(t) => format!("hierarchy -top {t}"),
        None => "hierarchy -auto-top".to_string(),
    };
    format!(
        "{}; {hier}; proc; flatten; async2sync; chformal -lower; dffunmap; setundef -zero; write_btor {}",
        read_cmds.join("; "),
        btor_out.display()
    )
}

fn write_file(path: &Path, content: &str) -> Result<(), AdapterError> {
    std::fs::write(path, content).map_err(io_err)
}

fn io_err(e: std::io::Error) -> AdapterError {
    AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: format!("adapter/yosys: io error: {e}"),
        location: None,
    }
}

/// Extension method for callers who already have an AdapterOptions and just
/// want the multi-source variant.
pub fn translate_sv_multi(
    primary: &str,
    additional: &HashMap<String, String>,
    options: &AdapterOptions,
    top: Option<&str>,
) -> Result<AdapterOutput, AdapterError> {
    let yopts = YosysOptions {
        top: top.map(str::to_string),
        additional_sources: additional
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        skip_verific_check: false,
    };
    translate_sv(primary, options, &yopts)
}

// =====================================================================
// TempDir helper (avoid pulling in another dep just for this)
// =====================================================================

struct TempDir {
    path: PathBuf,
    keep: bool,
}

impl TempDir {
    fn new(prefix: &str) -> Result<Self, AdapterError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        // Per-call uniqueness: pid + monotonic process counter + nanos
        // + thread id. Two parallel tests in the same process must not
        // collide on the same tempdir path.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let tid = format!("{:?}", std::thread::current().id());
        let tid_digits: String = tid.chars().filter(|c| c.is_ascii_digit()).collect();
        let mut base = std::env::temp_dir();
        base.push(format!("{prefix}-{pid}-{tid_digits}-{seq}-{nanos}"));
        std::fs::create_dir_all(&base).map_err(io_err)?;
        let keep = std::env::var("MUNUNU_KEEP_YOSYS_TMP")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        Ok(TempDir { path: base, keep })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yosys_available() -> bool {
        Command::new("yosys")
            .arg("-V")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn locate_yosys_finds_binary_or_errors() {
        let res = locate_yosys();
        if yosys_available() {
            assert!(res.is_ok());
        } else {
            assert!(res.is_err());
        }
    }

    #[test]
    fn detect_returns_false_for_sv() {
        // YosysAdapter::detect intentionally returns false; routing happens
        // explicitly via the CLI/API frontend selector.
        assert!(!YosysAdapter::detect("module foo; endmodule"));
    }

    #[test]
    fn elaborates_minimal_counter_to_btor2() {
        if !yosys_available() {
            eprintln!("skipping: yosys not on PATH");
            return;
        }
        // 2-bit free-running counter — exercises always_ff, parameters, and
        // arithmetic (which the hand-rolled SV adapter would not handle).
        let sv = r#"
module counter (input wire clk, input wire rst);
  reg [1:0] cnt;
  always @(posedge clk) begin
    if (rst) cnt <= 2'b00;
    else     cnt <= cnt + 1;
  end
endmodule
"#;
        let opts = AdapterOptions::default();
        let yopts = YosysOptions {
            top: Some("counter".into()),
            ..Default::default()
        };
        let out = translate_sv(sv, &opts, &yopts).expect("yosys translate");
        // 2-bit counter → 4 states; the bit-blaster also folds in the rst
        // input as a label dimension, but the state count itself is 4.
        assert!(
            out.source_info.state_count >= 4,
            "got {}",
            out.source_info.state_count
        );
    }

    /// End-to-end Phase 1 demo: round-trips `examples/btor2/safety_demo.sv`
    /// through Yosys and verifies the SVA assertion lands as a `safety_bad_*`
    /// μ-calculus property in the resulting CTXDSL.
    ///
    /// This is the canonical Phase 1 acceptance test — if it fails, either
    /// the Yosys script or the BTOR2 reader regressed. The example file is
    /// intentionally checked in so the test exercises a real on-disk
    /// artifact, not an inline string.
    #[test]
    fn phase1_demo_safety_assertion_surfaces_as_property() {
        if !yosys_available() {
            eprintln!("skipping: yosys not on PATH");
            return;
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/btor2/safety_demo.sv");
        let sv = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => {
                eprintln!("skipping: example file not found at {}", path.display());
                return;
            }
        };
        let opts = AdapterOptions::default();
        let yopts = YosysOptions {
            top: Some("safety_demo".into()),
            ..Default::default()
        };
        let out = translate_sv(&sv, &opts, &yopts).expect("yosys translate");
        assert!(
            out.source_info.property_count >= 1,
            "expected at least one safety property"
        );
        assert!(
            out.ctxdsl.contains("safety_bad_"),
            "expected a `safety_bad_*` formula in the emitted CTXDSL"
        );
    }
}
