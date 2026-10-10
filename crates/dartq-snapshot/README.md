# dartq-snapshot

Persistent snapshot substrate for dartq's cold-start path: analysis state
written to and restored from disk as content-addressed entries (hashing via
blake3, hex-encoded keys), with atime/mtime bookkeeping for LRU eviction.

## Role in the workspace

A standalone crate (no `dartq-*` dependencies); the daemon uses it to
persist session state across restarts, and higher layers decide what a
snapshot contains.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
