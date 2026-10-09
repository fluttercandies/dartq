//! dartq-side driver: invokes the dartq CLI as an external command and parses its machine-format
//! output (PRD P6 superset contract). The command line comes from the `--dartq-cmd` template
//! (see [`DartqDriver::from_template`]); process invocation, timeout handling, and machine parsing
//! are shared with the oracle side via [`crate::oracle::run_driver`]. Exit-code contract (facts §3,
//! dartdev): {0,1,2,3} are valid analysis exits (0 clean / 1 infos / 2 warnings / 3 errors) and the
//! parsed lines are the Jaccard input; 64 is a usage error and 4 a crash — both fail the harness
//! instead of producing a diagnostic set.

use std::path::Path;

use crate::machine::{MachineDiag, OutputFormat};
use crate::oracle::{DriverSide, run_driver};
/// dartq command driver: `program + args` are both given by the caller.
pub(crate) struct DartqDriver {
    program: String,
    args: Vec<String>,
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

    /// `format`: output format of the external command. The dartq contract is machine (PRD P6);
    /// in `--e2e-oracle` self-check mode the dartq side is dart analyze itself, so it must use the same format as the oracle side.
    pub(crate) fn analyze(
        &self,
        corpus_dir: &Path,
        format: OutputFormat,
    ) -> Result<Vec<MachineDiag>, String> {
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
        run_driver(DriverSide::Dartq, &self.program, &args, format)
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
        let diags = driver
            .analyze(&dir, OutputFormat::Machine)
            .unwrap_or_else(|e| panic!("cat mock failed: {e}"));

        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].code, "EXPECTED_TOKEN");
        assert_eq!(diags[0].message, "Expected to find ';'.");
        assert_eq!(diags[1].code, "UNUSED_LOCAL_VARIABLE");

        // Template without a placeholder: the corpus directory is appended as the last argument
        let driver = DartqDriver::from_template("/bin/cat /dev/null");
        let diags = driver
            .analyze(&dir, OutputFormat::Machine)
            .expect("cat /dev/null");
        assert!(diags.is_empty(), "empty stdout = no diags");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_command_is_clear_error() {
        let driver = DartqDriver::from_template("/nonexistent/dartq-binary-xyz check");
        let err = driver
            .analyze(Path::new("/tmp"), OutputFormat::Machine)
            .expect_err("missing binary must error");
        assert!(err.contains("failed to launch"), "error: {err}");
        assert!(err.contains("dartq"), "error names the side: {err}");
    }

    #[test]
    fn abnormal_exit_is_clear_error() {
        // exit 4 (crash tier, facts §3) is outside the valid set {0,1,2,3}
        let driver =
            DartqDriver::from_program("sh".to_string(), vec!["-c".into(), "exit 4".into()]);
        let err = driver
            .analyze(Path::new("/tmp"), OutputFormat::Machine)
            .expect_err("exit 4 must error");
        assert!(err.contains("Some(4)"), "error: {err}");
    }

    /// Exit 2 on the dartq side is the warnings tier (dartdev
    /// analyze.dart:719–733 `_Result`: 0 success / 1 infos / 2 warnings /
    /// 3 errors), not a usage error: `check` exits 2 on any warning under
    /// default flags, and that is a measurement — diagnostics ride stdout.
    #[test]
    fn warnings_exit_2_is_a_valid_measurement() {
        let driver =
            DartqDriver::from_program("sh".to_string(), vec!["-c".into(), "exit 2".into()]);
        let diags = driver
            .analyze(Path::new("/tmp"), OutputFormat::Machine)
            .expect("exit 2 is the warnings tier, not a usage error");
        assert!(diags.is_empty(), "empty stdout parses to an empty set");
    }

    /// 64 stays the usage tier on the dartq side too (dartdev.dart:210
    /// `EX_USAGE`): a malformed `--dartq-cmd` template would otherwise fold
    /// into "valid + empty output" and score a meaningless Jaccard 0.
    #[test]
    fn usage_exit_64_is_harness_error() {
        let driver =
            DartqDriver::from_program("sh".to_string(), vec!["-c".into(), "exit 64".into()]);
        let err = driver
            .analyze(Path::new("/tmp"), OutputFormat::Machine)
            .expect_err("usage exit 64 must error, not pass as an empty measurement");
        assert!(err.contains("Some(64)"), "error: {err}");
        assert!(err.contains("valid: 0-3"), "error names the valid set: {err}");
    }
}
