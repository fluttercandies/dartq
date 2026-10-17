# plugin-validation: real third-party plugin fixture

A vendored, pinned workspace for verifying that the dartq plugin host loads
real-world `analysis_server_plugin`-family lint plugins end to end, plus
the plugin-on/off A-B performance protocol (the `plugin-overhead` axis of
the benchmark suite).

Five plugins are vendored at exact pinned versions (all published within
weeks of the pin; re-verify before repinning):

| Package | Version | analyzer constraint |
| --- | --- | --- |
| leancode_lint | 27.0.0 | `>=13.0.0 <15.0.0` |
| clean_architecture_linter | 2.8.0 | `>=14.3.0 <15.0.0` |
| riverpod_lint | 3.1.9 | `>=13.0.0 <15.0.0` |
| dart_code_linter | 4.4.2 | `>=10.0.0 <15.0.0` |
| solid_lints | 2.0.0 | `^14.1.0` |

## Loading shape (per-plugin injection)

Each sidecar plugin process resolves imports through its own
`.dart_tool/package_config.json`. Each vendored plugin root therefore
carries three dartq-specific additions on top of the pristine pub-cache
copy:

- `dartq_plugin.json` — the plugin manifest the core discovers (name,
  version, entrypoint `bin/dartq_serve.dart`, capabilities, budgets).
- `bin/dartq_serve.dart` — the thin `startPlugin(plugin)` launch face,
  importing the package's own top-level `plugin` variable. Recipe from the
  in-repo `analysis_server_plugin` import shim's README
  (`crates/dartq-plugin-host/lib/src/analysis_server_plugin_shim`).
- `.dart_tool/package_config.json` — the injected config: the plugin maps
  to `..` (its vendored root), `analysis_server_plugin` maps to the
  in-repo shim, `dartq_plugin_host` maps to the host package, and every
  transitive dependency maps into `vendor/deps/<name>-<version>`.

Because each plugin gets its own process and its own config, per-plugin
analyzer constraints never meet a solver (all five currently resolve to
the same analyzer 14.5.0 line anyway — `vendor/deps` holds one copy per
distinct package version). Never run `dart pub get` inside a vendored
plugin root: pub would overwrite the injected config and the sidecar would
lose the shim mapping. `scripts/plugin-census.sh` fails loud if that
happens.

The dependency entries are lib-only: `test/`, `example/`, and
`extension/` trees were stripped from `vendor/deps/*` (execution reads
`lib/` only); the five plugin packages keep their full upstream trees —
their test corpora are the source of the trigger fixtures.

## Layout

- `vendor/<pkg>-<version>/` — pinned plugin roots (see above).
- `vendor/deps/<name>-<version>/` — shared transitive dependencies,
  version-suffixed dirs.
- `corpora/<pkg>/` — per-plugin trigger fixtures (one small file per rule;
  each corpus README records the upstream source and the parse-only
  caveats). The clean_architecture_linter corpus mirrors the feature-path
  layout its layer rules key on.
- `parity/<pkg>/` — per-plugin parity corpora (the plugin-parity oracle):
  one `analysis_options.yaml` declaring exactly that vendored plugin
  through the native `path:` source face (plus `plugins.dependency_overrides`
  pinning the official synthetic package's framework deps to vendored
  copies), a scratch `pubspec.yaml` for the dartq face's resolution, and a
  `lib/` mirroring the corpora triggers one file per pinned rule.
- `presets/absent/`, `presets/present/` — the two A-B arms: byte-identical
  `lib/` (materialized from `corpora/` by `scripts/plugin-ab.sh`) and
  functionally identical `pubspec.yaml` (identical `dependency_overrides`
  pointing at the vendored roots; only the package name/description differ);
  the only behavioral difference is the `plugins:` section in
  `analysis_options.yaml`.
- `expectations/` — census ground truth written by
  `scripts/plugin-census.sh` (serial phase).
- `tools/census_client.dart` — wire v1 census driver (plays the core's
  launch role against one plugin sidecar).
- `tools/ab_driver.dart` — dartq-RPC driver for one A-B arm: cold full
  analysis, incremental rounds, stats capture; emits one metrics JSON
  object. `--dump-diagnostics <file>` additionally records every
  `dartq/diagnostics` push payload as one JSON array of push params in
  arrival order — rows `{range, severity, code, source, message}`,
  zero-based lines / UTF-16 columns (dump contract: ab_driver.dart:37-40
  and :346-356; the parity comparator's dartq-side input; without the
  flag behavior is unchanged).
- `tools/parity_client.dart` — headless driver for the official analysis
  server snapshot over stdio: collects the merged server+plugin
  `analysis.errors` notifications as a JSON-lines event log (the parity
  comparator's official-side input).
- `results/` — A-B run outputs (gitignored).

## Serial-phase entry points

```sh
DARTQ_DART_SDK=<dart-sdk-root> scripts/plugin-census.sh   # census per plugin
DARTQ_DART_SDK=<dart-sdk-root> scripts/plugin-ab.sh 10    # A-B protocol
DARTQ_DART_SDK=<dart-sdk-root> \
  scripts/plugin-parity.sh <pkg> tests/plugin-validation/parity/<pkg> <out-dir>
                                                          # parity oracle arm
```

Both require the pinned SDK (Dart >= 3.13): the sidecars run plain `dart`
from PATH, so the SDK is prepended to PATH for driver, daemon, and every
spawned child. The A-B script also builds `dartq-cli` (release) and runs
one daemon instance at a time. Sample validity gate: any plugin stats line
with `restarts > 0` or `degraded_cycles > 0` flags the sample invalid
(restarts and drain degradation skew timings).

## Provenance

Vendored from pub.dev by exact version (`dart pub cache add <pkg>
--version <ver>`, copied out of the pub cache); dependencies resolved by a
per-plugin pub solve in a scratch package, then copied at their solved
versions. The vendored copies are verbatim upstream trees plus the three
dartq additions above; licenses and notices are retained.
