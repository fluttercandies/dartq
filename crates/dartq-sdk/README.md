# dartq-sdk

Dart SDK metadata for dartq: loading, identity, and querying of a SDK's
`dart:` library surface — which libraries exist, what files back them, and
under which language versions they are analyzed.

## Role in the workspace

Reads SDK layout over content from `dartq-source`, using `dartq-language`
for per-library language versions; consumed by `dartq-resolution` (core
library resolution) and by the engine.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
