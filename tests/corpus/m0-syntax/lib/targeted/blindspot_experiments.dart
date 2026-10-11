// Targets the experiment-gated family. The fasta parser reports these via
// reportExperimentNotEnabled (emission site:
// pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:2247, listener
// hook at forwarding_listener.dart:2396); which analyzer variant fires
// (experiment_not_enabled TSV:107 vs experiment_not_enabled_off_by_default
// TSV:108) depends on the flag table, and both are gate targets — either
// variant counts as exposure.
//
// Primary constructors: SHIPPED in 3.13 (experimental_features.yaml:200,
// enabledIn '3.13.0'), so this shape is valid at the corpus language
// version and neither side emits anything. Kept as the zero-exposure pin
// of the promotion — the experiment_not_enabled face for this feature is
// no longer reachable at 3.13 (the pre-review note claiming the feature
// was off by default is stale).
class PrimaryPoint(final int x) {}

// Augmentations: 'augment' without the feature. With the feature enabled
// the same shape reaches extension_augmentation_has_on_clause (TSV:110;
// fasta emit site pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:
// 3705, "Extension augmentations can't have 'on' clauses.").
augment extension AugmentedExtension on int {}

// Targets extraneous_modifier_in_primary_constructor (TSV:129; fasta emit
// site pkg/_fe_analyzer_shared/lib/src/parser/modifier_context.dart:718):
// 'covariant' is not a valid primary-constructor parameter modifier.
class PrimaryCovariant(covariant int x) {}

// Targets multiple_variance_modifiers (TSV:237; fasta emit site
// pkg/_fe_analyzer_shared/lib/src/parser/type_info_impl.dart:1506, "Each
// type parameter can have at most one variance modifier."): two variance
// modifiers on one type parameter. Variant fork: variance is still
// off-by-default at 3.13 (experimental_features.yaml:132, no enabledIn),
// so the oracle additionally emits experiment_not_enabled_off_by_default
// (TSV:108, reportExperimentNotEnabled, parser_impl.dart:2247) for this
// shape; dartq currently emits only multiple_variance_modifiers — a real
// gap, documented here rather than hidden.
class MultiVariance<out out T> {}

// Targets invalid_use_of_identifier_augmented (TSV:187): 'augmented' can
// only reference the augmented declaration inside an augmentation. Fires
// only when the augmentations feature is active on this library (the
// oracle's default configuration treats this as a plain identifier, so
// this shape may stay a documented gap for the oracle side).
int augmented = 1;
