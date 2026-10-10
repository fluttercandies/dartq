//! Shared test helpers.

mod errors;
mod escapes;
mod inserted;
mod kinds;
mod numbers_strings;
mod smoke;

use std::path::Path;

use crate::{ScanConfig, ScanResult, scan};
use dartq_diagnostics::Diagnostic;
use dartq_source::{FileId, FileTable};

pub(crate) fn config() -> ScanConfig {
    ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    }
}

pub(crate) fn config_off() -> ScanConfig {
    ScanConfig {
        enable_triple_shift: false,
        enable_augmentations: false,
    }
}

pub(crate) fn file_id() -> FileId {
    FileTable::new().intern(Path::new("test.dart"))
}

pub(crate) fn scan_src(src: &str) -> ScanResult {
    scan(src, config(), file_id())
}

pub(crate) fn codes(r: &ScanResult) -> Vec<&'static str> {
    r.diagnostics
        .iter()
        .map(|d: &Diagnostic| d.code.name)
        .collect()
}

/// Token kind sequence for a source (EOF excluded; features optionally off).
pub(crate) fn kinds_of(src: &str, cfg_off: bool) -> Vec<crate::TokenKind> {
    let r = if cfg_off {
        crate::scan(src, config_off(), file_id())
    } else {
        scan_src(src)
    };
    let mut v: Vec<crate::TokenKind> = r.tokens.as_slice().iter().map(|t| t.kind).collect();
    v.pop(); // EOF
    v
}

/// Convenience assertion: token kind sequence (EOF excluded).
#[track_caller]
pub(crate) fn assert_kinds(r: &ScanResult, expected: &[crate::TokenKind]) {
    let got: Vec<crate::TokenKind> = r.tokens.as_slice().iter().map(|t| t.kind).collect();
    let got_no_eof = &got[..got.len() - 1];
    assert_eq!(got_no_eof, expected, "kinds mismatch; diags={:?}", codes(r));
}

/// Convenience assertion: diagnostic code sequence.
#[track_caller]
pub(crate) fn assert_codes(r: &ScanResult, expected: &[&str]) {
    assert_eq!(codes(r), expected, "codes mismatch");
}
