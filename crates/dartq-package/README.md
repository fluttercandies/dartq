# dartq-package

The package identity model for dartq: the typed identity surface the
resolution layer consumes, composed on top of `dartq-pub`'s parsed
artifacts rather than raw maps and path strings.

## Role in the workspace

Built on `dartq-pub` and `dartq-source`; consumed by `dartq-resolution`
and by the engine, which keys per-package state on these identities.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
