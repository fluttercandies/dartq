# dartq-engine

The analysis engine facade: one `Engine` binds the immutable context
(content store, SDK snapshot, package identity, pub config); content
updates enqueue into a pending overlay, and `apply`/`analyze` run the
incremental gate, recompute dirty libraries in import-edge topological
order, and return the merged, canonically sorted diagnostic set.

## Role in the workspace

Assembles `dartq-scanner`, `dartq-parser`, and `dartq-resolution` into the
analysis pipeline and plugs semantic diagnosis in through the
`SemanticDiagnoser` seam (default: `dartq-semantic`'s diagnoser). It is the
face `dartq-commands` and `dartq-daemon` drive.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
