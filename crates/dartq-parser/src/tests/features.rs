//! Feature folding tests.

use crate::listener::ExperimentalFlag;
use crate::message::experiment_not_enabled_message;

use super::collector::latest_features;

#[test]
fn latest_version_enables_patterns_only() {
    let f = latest_features();
    assert!(f.patterns_enabled);
    // `variance` has no `enabledIn`: opt-in only.
    assert!(!f.variance_enabled);
    // Dot shorthands ship in 3.10.
    assert!(f.dot_shorthands_enabled);
}

/// The `handle_experiment_not_enabled` message selection
/// (`getExperimentNotEnabledMessage`, `errors.dart:12`): a shipped flag
/// (`enabledIn`) takes the `experiment_not_enabled` wording, an off-by-default
/// flag the `_off_by_default` one; both parameterize the yaml flag name. The
/// engine's diagnostic face renders the bare listener event through this, so
/// the selection is pinned here against the probed oracle rows (3.13.1:
/// `experiment_not_enabled_off_by_default` for variance,
/// `experiment_not_enabled` for dot-shorthands/primary-constructors).
#[test]
fn experiment_message_follows_the_flag_registry() {
    let text_of = |flag| {
        let m = experiment_not_enabled_message(flag);
        assert!(m.correction.is_none(), "{flag:?}");
        m
    };
    let m = text_of(ExperimentalFlag::Patterns);
    assert_eq!(m.code.name, "experiment_not_enabled");
    assert_eq!(
        m.text,
        "This requires the 'patterns' language feature to be enabled."
    );
    let m = text_of(ExperimentalFlag::PrimaryConstructors);
    assert_eq!(m.code.name, "experiment_not_enabled");
    assert_eq!(
        m.text,
        "This requires the 'primary-constructors' language feature to be enabled."
    );
    let m = text_of(ExperimentalFlag::Variance);
    assert_eq!(m.code.name, "experiment_not_enabled_off_by_default");
    assert_eq!(
        m.text,
        "This requires the experimental 'variance' language feature to be enabled."
    );
    let m = text_of(ExperimentalFlag::Augmentations);
    assert_eq!(m.code.name, "experiment_not_enabled_off_by_default");
    assert_eq!(
        m.text,
        "This requires the experimental 'augmentations' language feature to be enabled."
    );
}
