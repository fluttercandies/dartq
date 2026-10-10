# dartq-incremental

dartq's invalidation engine: signature hashes and dependency fingerprints
decide exactly which files re-analyze after an edit, so an incremental run
produces the same results as a full recomputation.

## Role in the workspace

A standalone crate (no `dartq-*` dependencies), kept embeddable so analysis
phases can contribute the fingerprints they own.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
