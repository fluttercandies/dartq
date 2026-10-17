# app-x slice golden — scaffold (pending the morning serial-phase pin)

This directory is the byte-stability pin for the corpus-gen `--flutter-app`
face (#2063). It is a SCAFFOLD: nothing is pinned yet. The morning
generation phase materializes the pin; `scripts/app-gate.sh` then verifies
it on every run (skip-if-absent until then).

## What gets pinned

Five files generated at `--scale slice --seed 0 --flutter-app`, hashed with
SHA-256 into `SHA256SUMS` (corpus-root-relative paths, `shasum -a 256`
format):

| file | why |
| --- | --- |
| `packages/flutter/lib/widgets.dart` | the in-corpus flutter stub library — the typed surface both the oracle and dartq resolve app code against |
| `packages/feature_0000/lib/feature_0000.dart` | first feature file (the barrel) |
| `packages/feature_0000/lib/src/facets/accel.dart` | second feature file (conditional-import hub) |
| `packages/feature_0000/lib/src/facets/accel_io.dart` | third feature file |
| `packages/feature_0000/lib/src/widgets/widget_00002.dart` | the first real widget host (StatefulWidget + State + nested build tree) |

The facet and widget-host files are index-derived (the face's emitters draw
nothing from the shared RNG stream), so their bytes are identical at every
scale; the barrel and the stub are pinned at slice scale (the gate's check
always regenerates at `--scale slice`, so the pin is self-consistent).

## Pin procedure (morning serial phase, paste-ready)

```bash
cargo run -q -p corpus-gen -- --out target/gate/app-x-slice --scale slice --seed 0 --flutter-app
(cd target/gate/app-x-slice && shasum -a 256 \
  packages/flutter/lib/widgets.dart \
  packages/feature_0000/lib/feature_0000.dart \
  packages/feature_0000/lib/src/facets/accel.dart \
  packages/feature_0000/lib/src/facets/accel_io.dart \
  packages/feature_0000/lib/src/widgets/widget_00002.dart) \
  > goldens/app-x/slice/SHA256SUMS
```

Regenerate deliberately: any corpus-gen change that moves these bytes must
re-pin this directory in the same change and say so in the ledger — never
hand-edit the hashes.
