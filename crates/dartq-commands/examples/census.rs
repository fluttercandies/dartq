//! The `dartq analyze --format=machine` diagnostic face without the CLI
//! shell: the same workspace assembly, plugin weave, analyze pass, and
//! machine writer the CLI face drives, addressed at one package
//! directory. Keeps differential census verification moving while the
//! CLI crate is mid-refactor in a parallel session — the rows are
//! byte-identical to the CLI face (same pipeline, same writer), which
//! the census runner cross-checks against the CLI binary's own output.

use std::io::Write;
use std::path::{Path, PathBuf};

use dartq_commands::analyze::Target;
use dartq_commands::workspace::{AnalyzeOverrides, engine_config_with_overrides};
use dartq_diagnostics::{AnalysisOutput, write_machine};

fn main() {
    let Some(target) = std::env::args().nth(1) else {
        eprintln!("usage: census <pkg-dir>");
        std::process::exit(2);
    };
    let code = run(&PathBuf::from(target));
    std::process::exit(code);
}

fn run(target: &Path) -> i32 {
    let Ok(dir) = target.canonicalize() else {
        eprintln!(
            "error: Directory or file doesn't exist: {}",
            target.display()
        );
        return 2;
    };
    let overrides = AnalyzeOverrides::default();
    let (config, root, _advisory) = match engine_config_with_overrides(&dir, None, &overrides) {
        Ok(faces) => faces,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    let mut plugins =
        dartq_commands::plugins::PluginSet::launch(&root, &config.pub_config, &config.config_dir);
    let mut engine = match dartq_engine::Engine::new(config) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("error: engine context unusable: {error}");
            plugins.shutdown();
            return 2;
        }
    };
    let plugin_ref = if plugins.is_empty() {
        None
    } else {
        Some(&mut plugins)
    };
    let run = match dartq_commands::analyze::run_targets(
        &mut engine,
        &[],
        std::slice::from_ref(&root),
        &[Target::Dir(dir)],
        plugin_ref,
    ) {
        Ok(run) => run,
        Err(error) => {
            eprintln!("dartq: internal error: {error}");
            plugins.shutdown();
            return 4;
        }
    };
    let output = AnalysisOutput {
        diagnostics: &run.diagnostics,
        files: &run.layout,
    };
    let mut out = std::io::stdout().lock();
    let _ = write_machine(&mut out, &output);
    let _ = out.flush();
    plugins.shutdown();
    0
}
