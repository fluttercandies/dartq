//! dartq-side driver: invokes the dartq CLI as an external command and parses its machine-format
//! output (PRD P6 superset contract). The command line comes from the `--dartq-cmd` template
//! (see [`DartqDriver::from_template`]); process invocation, timeout handling, and machine parsing
//! are shared with the oracle side via [`crate::oracle::run_driver`]. Exit-code contract (facts §3,
//! dartdev): {0,1,2,3} are valid analysis exits (0 clean / 1 infos / 2 warnings / 3 errors) and the
//! parsed lines are the Jaccard input; 64 is a usage error and 4 a crash — both fail the harness
//! instead of producing a diagnostic set.

use std::path::Path;

use crate::machine::{MachineDiag, OutputFormat};
use crate::oracle::{DriverOutcome, DriverSide, run_driver_classified};

/// dartq command driver: `program + args` are both given by the caller.
pub(crate) struct DartqDriver {
    program: String,
    args: Vec<String>,
}

/// Classification of one dartq driver invocation (see [`DartqDriver::probe`]).
#[derive(Debug)]
pub(crate) enum DartqProbe {
    /// Valid measurement exit (0-3); parsed diagnostics, possibly empty.
    Measured(Vec<MachineDiag>),
    /// Crash tier (exit 4) with empty stdout — the whole-batch `catch_unwind`
    /// shape the corpus auto-bisect attributes.
    Panic,
    /// Any other failure; the message is the historical harness-error text.
    Failed(String),
}

impl DartqDriver {
    /// Template looks like `cargo run -q -p dartq-cli -- check --format=machine`.
    /// Split on whitespace into argv (spaces inside paths do not break argv: `{dir}` replacement happens after splitting).
    /// If any argument contains the `{dir}` placeholder it is replaced with the corpus directory; otherwise the corpus directory is appended as the last argument.
    /// Adding `--no-fatal-warnings` makes both sides exit {0, 3} symmetrically
    /// (warnings no longer fatal; diagnostic output is unaffected — the oracle
    /// side already runs with it, see [`crate::oracle::OracleDriver`]).
    pub(crate) fn from_template(template: &str) -> Self {
        let mut parts = template.split_whitespace().map(str::to_owned);
        let program = parts.next().unwrap_or_default();
        Self {
            program,
            args: parts.collect(),
        }
    }

    /// Build directly from program/argv (`--e2e-oracle` and tests, bypassing template whitespace splitting).
    pub(crate) fn from_program(program: String, args: Vec<String>) -> Self {
        Self { program, args }
    }

    /// argv for one corpus dir: `{dir}` placeholder replacement, appending the
    /// dir when the template carries no placeholder.
    fn command_args(&self, corpus_dir: &Path) -> Vec<String> {
        let dir_str = corpus_dir.display().to_string();
        let has_placeholder = self.args.iter().any(|a| a.contains("{dir}"));
        let mut args: Vec<String> = self
            .args
            .iter()
            .map(|a| a.replace("{dir}", &dir_str))
            .collect();
        if !has_placeholder {
            args.push(dir_str);
        }
        args
    }

    /// One classified dartq invocation: valid exits (0-3) measure, the crash
    /// tier (exit 4 with empty stdout — a panicking whole-batch run) surfaces
    /// as [`DartqProbe::Panic`] so the corpus auto-bisect can key on it, and
    /// every other failure keeps the historical harness-error message.
    pub(crate) fn probe(&self, corpus_dir: &Path, format: OutputFormat) -> DartqProbe {
        match run_driver_classified(
            DriverSide::Dartq,
            &self.program,
            &self.command_args(corpus_dir),
            format,
        ) {
            DriverOutcome::Measured(diags) => DartqProbe::Measured(diags),
            DriverOutcome::Crash(_) => DartqProbe::Panic,
            DriverOutcome::Failed(msg) => DartqProbe::Failed(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// End to end (the mock executable exists only in this test): replays a fixed machine output via /bin/cat,
    /// exercising the full code path of `{dir}` template replacement and the parsing pipeline.
    #[test]
    fn cat_mock_replays_machine_output() {
        let dir = std::env::temp_dir().join(format!("dq_dartq_mock_{}", std::process::id()));
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            dir.join("machine.txt"),
            concat!(
                "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|/tmp/corpus/lib/a.dart|2|11|1|Expected to find ';'.\n",
                "WARNING|STATIC_WARNING|UNUSED_LOCAL_VARIABLE|/tmp/corpus/lib/a.dart|3|7|1|The value of the local variable 'x' isn't used.\n",
            ),
        )
        .expect("fixture");

        let driver = DartqDriver::from_template("/bin/cat {dir}/machine.txt");
        let DartqProbe::Measured(diags) = driver.probe(&dir, OutputFormat::Machine) else {
            panic!("cat mock must measure");
        };

        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].code, "EXPECTED_TOKEN");
        assert_eq!(diags[0].message, "Expected to find ';'.");
        assert_eq!(diags[1].code, "UNUSED_LOCAL_VARIABLE");

        // Template without a placeholder: the corpus directory is appended as the last argument
        let driver = DartqDriver::from_template("/bin/cat /dev/null");
        let DartqProbe::Measured(diags) = driver.probe(&dir, OutputFormat::Machine) else {
            panic!("cat /dev/null must measure");
        };
        assert!(diags.is_empty(), "empty stdout = no diags");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_command_is_clear_error() {
        let driver = DartqDriver::from_template("/nonexistent/dartq-binary-xyz check");
        let DartqProbe::Failed(err) = driver.probe(Path::new("/tmp"), OutputFormat::Machine) else {
            panic!("missing binary must fail");
        };
        assert!(err.contains("failed to launch"), "error: {err}");
        assert!(err.contains("dartq"), "error names the side: {err}");
    }

    /// Exit 4 with empty stdout is the crash tier (facts §3) AND the panic
    /// signature: the whole-batch `catch_unwind` shape the corpus auto-bisect
    /// attributes. A crash with leftover stdout is a plain failure instead
    /// (see `probe_classifies_measurement_panic_and_failure`).
    #[test]
    fn crash_exit_4_empty_stdout_is_panic_signature() {
        let driver =
            DartqDriver::from_program("sh".to_string(), vec!["-c".into(), "exit 4".into()]);
        assert!(matches!(
            driver.probe(Path::new("/tmp"), OutputFormat::Machine),
            DartqProbe::Panic
        ));
    }

    /// Exit 2 on the dartq side is the warnings tier (dartdev
    /// analyze.dart:719–733 `_Result`: 0 success / 1 infos / 2 warnings /
    /// 3 errors), not a usage error: `check` exits 2 on any warning under
    /// default flags, and that is a measurement — diagnostics ride stdout.
    #[test]
    fn warnings_exit_2_is_a_valid_measurement() {
        let driver =
            DartqDriver::from_program("sh".to_string(), vec!["-c".into(), "exit 2".into()]);
        let DartqProbe::Measured(diags) = driver.probe(Path::new("/tmp"), OutputFormat::Machine)
        else {
            panic!("exit 2 is the warnings tier, not a usage error");
        };
        assert!(diags.is_empty(), "empty stdout parses to an empty set");
    }

    /// 64 stays the usage tier on the dartq side too (dartdev.dart:210
    /// `EX_USAGE`): a malformed `--dartq-cmd` template would otherwise fold
    /// into "valid + empty output" and score a meaningless Jaccard 0.
    #[test]
    fn usage_exit_64_is_harness_error() {
        let driver =
            DartqDriver::from_program("sh".to_string(), vec!["-c".into(), "exit 64".into()]);
        let DartqProbe::Failed(err) = driver.probe(Path::new("/tmp"), OutputFormat::Machine) else {
            panic!("usage exit 64 must fail, not pass as an empty measurement");
        };
        assert!(err.contains("Some(64)"), "error: {err}");
        assert!(
            err.contains("valid: 0-3"),
            "error names the valid set: {err}"
        );
    }

    /// Probe classification: a clean exit measures, the crash signature
    /// (exit 4 + empty stdout — the whole-batch `catch_unwind` shape) surfaces
    /// as [`DartqProbe::Panic`], and everything else stays a plain failure.
    #[test]
    fn probe_classifies_measurement_panic_and_failure() {
        let measure =
            DartqDriver::from_program("sh".to_string(), vec!["-c".into(), "exit 0".into()]);
        assert!(matches!(
            measure.probe(Path::new("/tmp"), OutputFormat::Machine),
            DartqProbe::Measured(_)
        ));

        let panic = DartqDriver::from_program("sh".to_string(), vec!["-c".into(), "exit 4".into()]);
        assert!(matches!(
            panic.probe(Path::new("/tmp"), OutputFormat::Machine),
            DartqProbe::Panic
        ));

        // Crash with partial stdout is NOT the bisect signature: a plain failure.
        let partial = DartqDriver::from_program(
            "sh".to_string(),
            vec!["-c".into(), "echo leftover; exit 4".into()],
        );
        match partial.probe(Path::new("/tmp"), OutputFormat::Machine) {
            DartqProbe::Failed(msg) => assert!(msg.contains("Some(4)"), "error: {msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }

        // Usage exit 64: plain failure (never a bisect trigger).
        let usage =
            DartqDriver::from_program("sh".to_string(), vec!["-c".into(), "exit 64".into()]);
        match usage.probe(Path::new("/tmp"), OutputFormat::Machine) {
            DartqProbe::Failed(msg) => assert!(msg.contains("Some(64)"), "error: {msg}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
