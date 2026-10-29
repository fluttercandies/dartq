//! port_upstream — converts one upstream linter rule test into a
//! dartq-fixture data file.
//!
//! ```text
//! port_upstream <rule_test.dart> [out.fixture]
//! ```
//!
//! The upstream test file path is an argument (never baked in). The
//! converted fixture goes to `out.fixture`, or stdout when omitted. The
//! per-file tally — ported vs skipped, each skip with its reason — always
//! goes to stderr.

use std::io::{Read, Write};

fn main() {
    let mut args = std::env::args_os().skip(1);
    let (input, output) = match (args.next(), args.next(), args.next()) {
        (Some(input), output, None) => (input, output),
        _ => {
            eprintln!("usage: port_upstream <rule_test.dart> [out.fixture]");
            std::process::exit(2);
        }
    };
    let mut source = String::new();
    match std::fs::File::open(&input) {
        Ok(mut file) => {
            if let Err(e) = file.read_to_string(&mut source) {
                eprintln!(
                    "port_upstream: cannot read {}: {e}",
                    input.to_string_lossy()
                );
                std::process::exit(2);
            }
        }
        Err(e) => {
            eprintln!(
                "port_upstream: cannot open {}: {e}",
                input.to_string_lossy()
            );
            std::process::exit(2);
        }
    }
    let outcome = match dartq_fixture::port_file(&source) {
        Ok(outcome) => outcome,
        Err(reason) => {
            eprintln!(
                "port_upstream: {}: rejected whole file: {reason}",
                input.to_string_lossy()
            );
            std::process::exit(1);
        }
    };
    let rendered = dartq_fixture::render(&outcome);
    match output {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, rendered) {
                eprintln!(
                    "port_upstream: cannot write {}: {e}",
                    path.to_string_lossy()
                );
                std::process::exit(2);
            }
        }
        None => {
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(rendered.as_bytes());
            let _ = stdout.flush();
        }
    }
    let (ported, skipped) = outcome.tally();
    eprintln!(
        "port_upstream: {} rule `{}`: {ported} ported, {skipped} skipped/rejected",
        input.to_string_lossy(),
        outcome.rule
    );
    for (test, reason) in &outcome.skipped {
        eprintln!("  skipped {test}: {reason}");
    }
}
