//! Corpus density metrics (layout spec §1.5, risk R2): scan the m0-syntax
//! corpus with the dartiq scanner, drive the builder with the spec's
//! structural node model (0.35-0.45 nodes/token, midpoint; ~1 extra pool
//! word per node), and assert the two hard ceilings:
//!
//! 1. ≤30 B average per node — measured on the real built tables.
//! 2. ≤1.5 MB of AST per MB of source — evaluated at the budget's
//!    definition point (spec §1.2: SDK-average 8.0 B/token ≈ 125k
//!    tokens/MB, ×0.40 nodes/token = 50k nodes/MB). The m0-syntax corpus is
//!    a syntax-coverage set whose token density is far above the SDK
//!    average, so its raw per-source-byte number is reported for reference
//!    only.
//!
//! The node/token ratio is the one soft number in the model; the parser
//! milestones re-validate it against real ASTs, while this test pins the
//! data layer's per-node cost.
//!
//! Density history: the pre-#151 model (token leaves stored their token as
//! payload word 0) measured **27.86 B/node** and **1.251 MB/MB** on this
//! corpus. The #151 contract stores no payload word for token leaves or
//! span-only kinds (Arguments' lparen is its span head — derivable-word0);
//! its model cycled 0/2/0/2/0/0/0/0 pool words (4 per 9 nodes) ⇒ 24 +
//! 16/9 ≈ **25.78 B/node** and 0.40 × 25.78 / 8.0 ≈ **1.29 MB/MB**. The
//! #174 contract additionally eliminated the Identifier token word (span
//! head) and the ModSet words (header flag bits); the model below cycles
//! 0/1/0/2/0/0/0/0 pool words over its 9-node chunks — 3 words per 9 nodes
//! — so the arithmetic expectation is 24 + 12/9 ≈ **25.33 B/node**
//! steady-state and ≈ 0.40 × 25.33 / 8.0 ≈ **1.27 MB/MB** at the budget's
//! definition point — confirmed by the serial-phase metrics run.

#![allow(clippy::cast_possible_truncation)] // ids/offsets are u32 by design
#![allow(clippy::cast_precision_loss)] // measurement math only

use std::fs;
use std::path::{Path, PathBuf};

use dartiq_ast::{AstBuilder, NodeKind};
use dartiq_scanner::{ScanConfig, TokenTable, scan};
use dartiq_source::FileTable;

/// Midpoint of the structural estimate 0.35-0.45 nodes/token (layout spec §1.2).
const NODES_PER_TOKEN: f64 = 0.40;

/// SDK-average bytes per token measured over 370 real SDK files (6.6 MB,
/// layout spec §1.2) — the token-density anchor the MB/MB budget is defined
/// against (~125k tokens per MB of source ⇒ 50k nodes per MB).
const BYTES_PER_TOKEN_SDK_AVERAGE: f64 = 8.0;

const CONFIG: ScanConfig = ScanConfig {
    enable_triple_shift: true,
    enable_augmentations: true,
};

fn collect_dart_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_dart_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "dart") {
            out.push(path);
        }
    }
}

/// Builds the model AST for one file: chunks of one span-only container plus
/// 8 leaf-slot events whose payload widths cycle 0/1/0/2/0/0/0/0 — 3 words
/// per 9 nodes. Post-#174: token leaves (LiteralInt/Bool/Null) and span-only
/// kinds (StringLiteral, Send, Arguments) store no payload word, and the
/// Identifier token is derived from the span head; the words that remain are
/// real fields (Identifier ctx, Conditional question/colon), built via
/// begin/patch/end.
fn model_ast(tokens: &TokenTable) -> dartiq_ast::SyntaxTable {
    let token_count = tokens.len() as u32;
    #[allow(clippy::cast_sign_loss)] // the model count is positive by construction
    let chunks = ((tokens.len() as f64) * NODES_PER_TOKEN / 9.0) as usize;
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    let mut token = 0u32;
    for _ in 0..chunks {
        b.begin(NodeKind::Block, token);
        // TokenLeaf: 0 words.
        b.leaf(NodeKind::LiteralInt, token);
        token = (token + 1) % token_count.max(1);
        // Fielded Identifier { ctx }: 1 word (the token is the span head,
        // #174), begin/patch/end.
        b.begin(NodeKind::Identifier, token);
        b.patch(0, 0);
        b.end(NodeKind::Identifier, token + 1);
        token = (token + 1) % token_count.max(1);
        // TokenLeaf: 0 words.
        b.leaf(NodeKind::LiteralBool, token);
        token = (token + 1) % token_count.max(1);
        // Fielded Conditional { question, colon }: 2 words.
        b.begin(NodeKind::Conditional, token);
        b.patch(0, token);
        b.patch(1, token);
        b.end(NodeKind::Conditional, token + 1);
        token = (token + 1) % token_count.max(1);
        // TokenLeaf: 0 words.
        b.leaf(NodeKind::LiteralNull, token);
        token = (token + 1) % token_count.max(1);
        // SpanOnly: 0 words.
        b.leaf(NodeKind::StringLiteral, token);
        token = (token + 1) % token_count.max(1);
        // SpanOnly leaf_span: 0 words.
        b.leaf_span(NodeKind::Send, token, token + 1);
        token = (token + 1) % token_count.max(1);
        // SpanOnly: 0 words — lparen is the span head (#151 derivable-word0),
        // rparen the last span token.
        b.leaf(NodeKind::Arguments, token);
        token = (token + 1) % token_count.max(1);
        b.end(NodeKind::Block, token);
    }
    b.end(NodeKind::CompilationUnit, token_count);
    b.finish()
}

#[test]
fn corpus_ast_density_within_budget() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/m0-syntax");
    let mut files = Vec::new();
    collect_dart_files(&corpus, &mut files);
    files.sort();
    assert!(
        files.len() >= 10,
        "corpus sample too small: {} files",
        files.len()
    );

    let mut files_table = FileTable::new();
    let mut total_source = 0usize;
    let mut total_tokens = 0usize;
    let mut total_nodes = 0usize;
    let mut total_footprint = 0usize;

    for path in &files {
        let source =
            fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let result = scan(&source, CONFIG, files_table.intern(path));
        let ast = model_ast(&result.tokens);

        total_source += source.len();
        total_tokens += result.tokens.len();
        total_nodes += ast.len();
        total_footprint += ast.memory_footprint();
    }

    assert!(
        total_source >= 100 * 1024,
        "corpus sample too small: {total_source} bytes"
    );

    // Hard ceiling 1: per-node cost, measured on the built tables.
    let bytes_per_node = total_footprint as f64 / total_nodes as f64;
    assert!(
        bytes_per_node <= 30.0,
        "average node cost {bytes_per_node:.2} B exceeds the 30 B cap"
    );

    // Hard ceiling 2: the MB/MB budget at its definition point —
    // nodes/token × B/token × tokens/MB(spec) = AST MB per source MB.
    let nodes_per_token = total_nodes as f64 / total_tokens as f64;
    let density = nodes_per_token * bytes_per_node * (1_048_576.0 / BYTES_PER_TOKEN_SDK_AVERAGE)
        / 1_048_576.0;
    assert!(
        density <= 1.5,
        "AST density {density:.3} MB/MB exceeds the 1.5 MB/MB cap"
    );

    // Reference only: this corpus is far denser in tokens than the SDK
    // average (syntax-coverage files), so its raw number exceeds the budget
    // without saying anything about the layer.
    let corpus_density = total_footprint as f64 / total_source as f64;
    let corpus_bytes_per_token = total_source as f64 / total_tokens as f64;

    println!("corpus files:              {}", files.len());
    println!("source bytes:              {total_source}");
    println!("scanned tokens:            {total_tokens}");
    println!(
        "corpus token density:      {corpus_bytes_per_token:.2} source B/token (SDK average 8.0)"
    );
    println!("model nodes:               {total_nodes} ({nodes_per_token:.3} nodes/token)");
    println!("AST footprint:             {total_footprint} bytes");
    println!("average node cost:         {bytes_per_node:.2} B/node (cap 30)");
    println!("AST density (spec model):  {density:.3} MB per MB of source (cap 1.5)");
    println!("AST density (raw corpus):  {corpus_density:.3} MB per MB (reference only)");
}
