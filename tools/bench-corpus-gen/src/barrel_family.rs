//! Family B — barrel-import-chain libraries (the resolution/library-graph
//! stress face).
//!
//! Each chain is a hub-and-spine export tree: hub modules declare public
//! names, the spine levels re-export the previous level plus two more hubs,
//! and importer files at the leaves pull the root barrel and touch names
//! that flow up through every spine level. Hubs re-exported at two different
//! spine levels flow the same elements through multiple export paths — the
//! overlapping re-export shape that stresses export-scope fixpoints and
//! per-library import-closure BFS — without ever exporting two distinct
//! elements under one name (which would be a real ambiguity error).
//!
//! Export rule guaranteeing importer references always resolve: every hub
//! export, whether a `show` combinator or a full export, includes that
//! hub's `Item` class and `tag` function. `Batch` is referenced only for
//! even-indexed hubs, which are exactly the fully-exported ones.
//!
//! Naming contract: chain and hub numbers are zero-padded to two digits in
//! both import URIs and emitted identifiers, so the two always agree.

use crate::rng::SplitMix64;
use crate::{File, sub};

/// Hub modules per chain.
pub const HUBS: usize = 12;
/// Spine levels per chain (the root barrel is `barrel{DEPTH-1}`).
pub const DEPTH: usize = 12;
/// Nominal lines an importer file aims for.
const IMPORTER_TARGET: usize = 170;
/// A chain stops attracting importers at this nominal size.
const CHAIN_NOMINAL: usize = 2300;
/// Below this remaining budget a fresh chain is not started. Sized to the
/// chain minimum: hub+spine fixed cost (~480 lines) plus one importer
/// (~170), so the final chain absorbs the budget tail instead of leaving
/// the family under its share.
const CHAIN_FLOOR: usize = 650;

const LABELS: [&str; 6] = ["first", "second", "row", "core", "grid", "node"];

fn pad2(n: usize) -> String {
    format!("{n:02}")
}

fn hub_file(chain: usize, hub: usize, label: &str) -> String {
    let binds = [
        ("::c", pad2(chain)),
        ("::h", pad2(hub)),
        ("::l", label.to_string()),
    ];
    sub(
        "\
// Barrel-chain hub (chain ::c, hub ::h): declares the names the spine
// re-exports — some through `show` combinators, some fully, some through
// several spine levels at once.

class Item::c_::h {
  final int id;
  final String label;

  const Item::c_::h(this.id, this.label);

  int doubled() => id * 2;

  @override
  String toString() => 'Item::c_::h($id, ::l)';
}

class Batch::c_::h {
  final List<Item::c_::h> entries;

  const Batch::c_::h(this.entries);

  int sum() {
    var acc = 0;
    for (final e in entries) {
      acc += e.id;
    }
    return acc;
  }
}

String tag::c_::h(Item::c_::h item) => '${item.label}#::l';

",
        &binds,
    )
}

fn spine_file(chain: usize, level: usize) -> String {
    let (full, shown) = if level == 0 {
        (0, 1)
    } else {
        ((2 * level) % HUBS, (2 * level + 1) % HUBS)
    };
    let binds = [
        ("::c", pad2(chain)),
        ("::d", pad2(level)),
        ("::p", pad2(level.saturating_sub(1))),
        ("::a", pad2(full)),
        ("::b", pad2(shown)),
    ];
    let mut body = if level == 0 {
        sub(
            "\
// Barrel spine root of chain ::c: exports the first hub pair, with the
// shown hub flowing only its `Item` and `tag` names upward.

",
            &binds,
        )
    } else {
        sub(
            "\
// Barrel spine level ::d of chain ::c: re-exports the previous level plus
// two hub modules, so the root barrel flows overlapping name sets through
// several export paths at once.

",
            &binds,
        )
    };
    if level > 0 {
        body.push_str(&sub("export 'barrel::p.dart';\n\n", &binds));
    }
    body.push_str(&sub("export 'hub::a.dart';\n", &binds));
    body.push_str(&sub(
        "export 'hub::b.dart' show Item::c_::b, tag::c_::b;\n\n",
        &binds,
    ));
    body
}

const GATE_BLOCK: &str = "\
class Gate::c_::i {
  final Item::c_::h1 entry;
  final Batch::c_::h2 pool;

  const Gate::c_::i(this.entry, this.pool);

  int score() => entry.doubled() + pool.sum();
}

";

const RACK_BLOCK: &str = "\
class Rack::c_::i {
  final List<Item::c_::h1> slots;

  const Rack::c_::i(this.slots);

  int packed() {
    var acc = 0;
    for (final slot in slots) {
      acc += slot.doubled();
    }
    return acc;
  }
}

";

const VAULT_BLOCK: &str = "\
class Vault::c_::i_::n {
  final Map<String, Item::c_::h1> byLabel;

  const Vault::c_::i_::n(this.byLabel);

  int weightOf(String label) => byLabel[label]?.doubled() ?? -1;
}

";

const DEPOT_BLOCK: &str = "\
class Depot::c_::i_::n {
  final Batch::c_::h2 spare;

  const Depot::c_::i_::n(this.spare);

  int audited() {
    var acc = 0;
    for (final item in spare.entries) {
      acc += item.doubled() + item.label.length;
    }
    return acc;
  }
}

";

const STAT_BLOCK: &str = "\
int stat::c_::i_::n(Item::c_::h1 item) => tag::c_::h1(item).length + item.id;

";

/// Consumer blocks an importer can be padded with, in rotation order.
const FILL_BLOCKS: [&str; 3] = [VAULT_BLOCK, DEPOT_BLOCK, STAT_BLOCK];

fn importer_file(chain: usize, index: usize, rng: &mut SplitMix64) -> String {
    let h1 = pad2(index % HUBS);
    // Even hub: `Batch` is only guaranteed to flow for full-exported hubs.
    let h2 = pad2((2 * index + 2) % HUBS);
    let binds = [
        ("::c", pad2(chain)),
        ("::i", index.to_string()),
        ("::h1", h1),
        ("::h2", h2),
        ("::r", pad2(DEPTH - 1)),
        ("::m", pad2(DEPTH / 2 - 1)),
    ];
    let label_a = LABELS[rng.below(LABELS.len() as u64) as usize];
    let label_b = LABELS[rng.below(LABELS.len() as u64) as usize];
    let mut body = sub(
        "\
// Barrel-chain importer (chain ::c, importer ::i): pulls the root barrel
// and touches names that flow up through every spine level, forcing the
// whole import closure to resolve.

import 'package:corpus100k/src/chains/chain::c/barrel::r.dart';

",
        &binds,
    );
    if index % 3 == 0 {
        body.push_str(&sub(
            "// The mid barrel re-flows the same elements: namespace merge stress.\nimport 'package:corpus100k/src/chains/chain::c/barrel::m.dart';\n\n",
            &binds,
        ));
    }
    body.push_str(&sub(
        "typedef Judge::c_::i = int Function(Item::c_::h1 item);\n\n",
        &binds,
    ));
    // Gate and Rack are unconditional: the closing total() references both.
    body.push_str(&sub(GATE_BLOCK, &binds));
    body.push_str(&sub(RACK_BLOCK, &binds));
    let mut block = 0usize;
    while crate::count_lines(&body) < IMPORTER_TARGET.saturating_sub(30) {
        let piece = FILL_BLOCKS[block % FILL_BLOCKS.len()];
        let mut piece_binds: Vec<(&str, String)> = binds.to_vec();
        piece_binds.push(("::n", block.to_string()));
        body.push_str(&sub(piece, &piece_binds));
        block += 1;
    }
    body.push_str(&sub(
        &format!(
            "\
final seeded::c_::i = <Item::c_::h1>[
  Item::c_::h1(1, '{label_a}'),
  Item::c_::h1(2, '{label_b}'),
];

final spare::c_::i = <Item::c_::h2>[Item::c_::h2(4, '{label_a}')];

String describe::c_::i(Item::c_::h1 item) => tag::c_::h1(item);

int weigh::c_::i(Judge::c_::i judge, Item::c_::h1 item) => judge(item);

int total::c_::i() {{
  final gate = Gate::c_::i(seeded::c_::i.first, Batch::c_::h2(spare::c_::i));
  return gate.score() + Rack::c_::i(seeded::c_::i).packed() + spare::c_::i.length;
}}

"
        ),
        &binds,
    ));
    body
}

/// Renders whole barrel chains until `budget_lines` is met. Every chain
/// carries its full hub/spine structure; the importer count adapts to the
/// remaining budget.
pub fn render_barrel_family(budget_lines: usize, rng: &mut SplitMix64) -> (Vec<File>, usize) {
    let mut files = Vec::new();
    let mut lines = 0usize;
    let mut chain = 0usize;
    while lines < budget_lines {
        let remaining = budget_lines - lines;
        if remaining < CHAIN_FLOOR {
            break;
        }
        let chain_budget = remaining.min(CHAIN_NOMINAL);
        let mut chain_lines = 0usize;
        for hub in 0..HUBS {
            let label = LABELS[rng.below(LABELS.len() as u64) as usize];
            let body = hub_file(chain, hub, label);
            chain_lines += crate::count_lines(&body);
            files.push(File {
                path: format!("lib/src/chains/chain{chain:02}/hub{hub:02}.dart"),
                body,
            });
        }
        for level in 0..DEPTH {
            let body = spine_file(chain, level);
            chain_lines += crate::count_lines(&body);
            files.push(File {
                path: format!("lib/src/chains/chain{chain:02}/barrel{level:02}.dart"),
                body,
            });
        }
        let mut importer = 0usize;
        while chain_lines < chain_budget {
            let body = importer_file(chain, importer, rng);
            chain_lines += crate::count_lines(&body);
            files.push(File {
                path: format!("lib/src/chains/chain{chain:02}/importer{importer:03}.dart"),
                body,
            });
            importer += 1;
        }
        lines += chain_lines;
        chain += 1;
    }
    (files, lines)
}
