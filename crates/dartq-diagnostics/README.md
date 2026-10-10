# dartq-diagnostics

dartq's diagnostic data model and oracle-exact output writers. Defines the
diagnostic values every phase emits and renders them in exactly the shapes
`dart analyze` produces — the property the differential-testing harness
relies on when comparing output byte for byte.

## Role in the workspace

Depends only on `dartq-source` (file identity and line indexes) and is
consumed by every diagnostic-producing phase (scanner, parser, flow,
semantic, lints) and by the engine, command, and daemon faces. Syntax and
semantic diagnostic codes are pinned here against checked-in data tables.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
