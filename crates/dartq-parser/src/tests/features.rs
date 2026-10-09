//! Feature folding tests.

use super::collector::latest_features;

#[test]
fn latest_version_enables_patterns_only() {
    let f = latest_features();
    assert!(f.patterns_enabled);
    // `variance` has no `enabledIn`: opt-in only.
    assert!(!f.variance_enabled);
}
