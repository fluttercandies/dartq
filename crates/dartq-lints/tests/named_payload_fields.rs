//! Named-field hygiene pin: rule production code (`src/rules/`) never
//! reads payload words positionally — every read goes through
//! `Check::field`/`field_token` (or `SyntaxTable::field`), resolved
//! against `dartq_ast::fields::PAYLOAD_FIELDS` by node kind. Fixture
//! builders and the engine's own raw face live outside the scan, so
//! no exemption list is needed.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

/// The banned raw-read call forms (both address words positionally).
const BANNED: [&str; 2] = ["payload(", "payload_word("];

fn rust_files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            rust_files_under(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    out.sort();
}

#[test]
fn rules_tree_has_no_raw_payload_reads() {
    let rules_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/rules");
    let mut files = Vec::new();
    rust_files_under(&rules_dir, &mut files);
    assert!(
        files.len() > 150,
        "scan found only {} files under {} — broken layout?",
        files.len(),
        rules_dir.display()
    );

    let mut offenders: Vec<String> = Vec::new();
    for file in &files {
        let text =
            fs::read_to_string(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        for (idx, line) in text.lines().enumerate() {
            if let Some(banned) = BANNED.iter().find(|b| line.contains(*b)) {
                let rel = file.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap_or(file);
                offenders.push(format!(
                    "{}:{}: [{}] {}",
                    rel.display(),
                    idx + 1,
                    banned,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "raw payload reads in rule production code (use \
         Check::field/field_token with a PayloadField name):\n  {}",
        offenders.join("\n  ")
    );
}
