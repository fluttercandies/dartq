//! dartiq-side driver: invokes `dartiq check` as an external command (machine output, PRD P6 superset contract).
//! dartiq's check subcommand is not implemented yet; the driver is parameterized by a command template,
//! so integration only requires swapping the template. The process invocation and machine parsing paths
//! are fully implemented in this task -- mock executables appear only in tests.

use std::path::Path;

use crate::machine::{MachineDiag, OutputFormat};
use crate::oracle::{DriverSide, run_driver};
/// dartiq command driver: `program + args` are both given by the caller.
pub(crate) struct DartiqDriver {
    program: String,
    args: Vec<String>,
}

impl DartiqDriver {
    /// Template looks like `cargo run -q -p dartiq-cli -- check --format=machine`.
    /// Split on whitespace into argv (spaces inside paths do not break argv: `{dir}` replacement happens after splitting).
    /// If any argument contains the `{dir}` placeholder it is replaced with the corpus directory; otherwise the corpus directory is appended as the last argument.
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

    /// `format`: output format of the external command. The dartiq contract is machine (PRD P6);
    /// in `--e2e-oracle` self-check mode the dartiq side is dart analyze itself, so it must use the same format as the oracle side.
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
        run_driver(DriverSide::Dartiq, &self.program, &args, format)
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
        let dir = std::env::temp_dir().join(format!("dq_dartiq_mock_{}", std::process::id()));
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            dir.join("machine.txt"),
            concat!(
                "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|/tmp/corpus/lib/a.dart|2|11|1|Expected to find ';'.\n",
                "WARNING|STATIC_WARNING|UNUSED_LOCAL_VARIABLE|/tmp/corpus/lib/a.dart|3|7|1|The value of the local variable 'x' isn't used.\n",
            ),
        )
        .expect("fixture");

        let driver = DartiqDriver::from_template("/bin/cat {dir}/machine.txt");
        let diags = driver
            .analyze(&dir, OutputFormat::Machine)
            .unwrap_or_else(|e| panic!("cat mock failed: {e}"));

        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].code, "EXPECTED_TOKEN");
        assert_eq!(diags[0].message, "Expected to find ';'.");
        assert_eq!(diags[1].code, "UNUSED_LOCAL_VARIABLE");

        // Template without a placeholder: the corpus directory is appended as the last argument
        let driver = DartiqDriver::from_template("/bin/cat /dev/null");
        let diags = driver
            .analyze(&dir, OutputFormat::Machine)
            .expect("cat /dev/null");
        assert!(diags.is_empty(), "empty stdout = no diags");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_command_is_clear_error() {
        let driver = DartiqDriver::from_template("/nonexistent/dartiq-binary-xyz check");
        let err = driver
            .analyze(Path::new("/tmp"), OutputFormat::Machine)
            .expect_err("missing binary must error");
        assert!(err.contains("failed to launch"), "error: {err}");
        assert!(err.contains("dartiq"), "error names the side: {err}");
    }

    #[test]
    fn abnormal_exit_is_clear_error() {
        // exit 4 (crash tier, facts §3) is outside the valid set {0,1,2,3}
        let driver =
            DartiqDriver::from_program("sh".to_string(), vec!["-c".into(), "exit 4".into()]);
        let err = driver
            .analyze(Path::new("/tmp"), OutputFormat::Machine)
            .expect_err("exit 4 must error");
        assert!(err.contains("Some(4)"), "error: {err}");
    }

    /// Exit 2 on the dartiq side is a usage error of the dartiq CLI itself
    /// (e.g. a malformed `--dartiq-cmd` template): folding it into
    /// "valid + empty output" would score a meaningless Jaccard 0, so it is a
    /// harness error like 4/64.
    #[test]
    fn usage_exit_2_is_harness_error() {
        let driver =
            DartiqDriver::from_program("sh".to_string(), vec!["-c".into(), "exit 2".into()]);
        let err = driver
            .analyze(Path::new("/tmp"), OutputFormat::Machine)
            .expect_err("usage exit 2 must error, not pass as an empty measurement");
        assert!(err.contains("Some(2)"), "error: {err}");
        assert!(err.contains("usage"), "error names the exit tier: {err}");
    }
}
