# dartq-semantic

The semantic element model: what each compilation unit declares (top-level
names, class members, formal tables, declared types), per-library member
enumeration and scoped lookup, reference diagnosis (the
`undefined_*`/`duplicate_definition` and `wrong_number_of_*` families), and
the canonical blake3 API signature the L2 incremental gate compares.

## Role in the workspace

A collecting listener folds `dartq-parser`'s event stream into the model;
`dartq-resolution` supplies the public-scope face and `dartq-flow` the
reachability facts the diagnosis pass records. `dartq-engine` ships the
default semantic diagnoser over this crate's `diagnose` module.

Part of the dartq workspace — see the repository root README for the
overall architecture and license terms.
