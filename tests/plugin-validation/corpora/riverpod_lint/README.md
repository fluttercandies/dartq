# riverpod_lint corpus

Trigger fixtures for the vendored `riverpod_lint` 3.1.9 plugin
(`vendor/riverpod_lint-3.1.9`). The published package carries no test
corpus (its suites live in the upstream monorepo, outside the pub
tarball), so triggers are shaped from the vendored rule sources
(`lib/src/lints/*.dart`): one file per rule, each reproducing the flagged
form the rule's visitor targets.

Every registered rule (see `lib/main.dart`'s `register` — 15
`registerWarningRule` calls) gets one minimal trigger; the package's
`registerAssist`/`registerFixForRule` entries are assists/fixes, not
diagnostic codes, and are out of scope for a lint corpus.

| File | Rule |
| --- | --- |
| `async_value_nullable_pattern.dart` | `async_value_nullable_pattern` |
| `avoid_build_context_in_providers.dart` | `avoid_build_context_in_providers` |
| `avoid_public_notifier_properties.dart` | `avoid_public_notifier_properties` |
| `avoid_ref_inside_state_dispose.dart` | `avoid_ref_inside_state_dispose` |
| `functional_ref.dart` | `functional_ref` |
| `missing_provider_scope.dart` | `missing_provider_scope` |
| `notifier_build.dart` | `notifier_build` |
| `notifier_extends.dart` | `notifier_extends` |
| `only_use_keep_alive_inside_keep_alive.dart` | `only_use_keep_alive_inside_keep_alive` |
| `protected_notifier_properties.dart` | `protected_notifier_properties` |
| `provider_dependencies.dart` | `provider_dependencies` |
| `provider_parameters.dart` | `provider_parameters` |
| `riverpod_syntax_error.dart` | `riverpod_syntax_error` |
| `scoped_providers_should_specify_dependencies.dart` | `scoped_providers_should_specify_dependencies` |
| `unsupported_provider_value.dart` | `unsupported_provider_value` |

Coverage: **15 / 15 registered warning codes**. None skipped — all 15 have
a rule source the flagged form is derived from.

Known caveats (expected for this package, recon R2):

- The rules type their subjects through `riverpod_analyzer_utils`, which is
  resolution-flavored; under the host's parse-only execution a subset will
  not fire. The files therefore carry no riverpod imports — `Provider`,
  `Notifier`, `ProviderScope`, `runApp`, `Theme`, `ref`, and `context` are
  free names/inline stand-ins, so every file parses standalone and the
  parse-only boundary (not a missing import) is what decides firing.
- `riverpod_syntax_error` surfaces riverpod-generator analyzer errors
  (`{0}`-placeholder code); its trigger reproduces a notifier missing its
  `build` method, the surface the rule forwards.
- Ground truth comes from the live census run
  (`scripts/plugin-census.sh`) plus the differential; rules skipped as
  parse-ineligible are recorded as known-different, not failures.
