# dartq-cache

dartq's layered cache: L1/L2/L3 tiers and their eviction policies, for
keeping analysis artifacts warm across runs and contexts.

## Role in the workspace

A standalone crate (no `dartq-*` dependencies); higher layers choose what
to cache at each tier.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
