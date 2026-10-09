//! Microbenchmark for the dartiq-types hot paths: canonical interning, the
//! subtype relation, the least upper bound, and the resident footprint of an
//! interned type population.
//!
//! Run (from the repository root):
//!
//! ```text
//! cargo run -p dartiq-types --example microbench --release
//! ```
//!
//! Methodology (also printed by the binary):
//!
//! - `--release` is required; debug numbers are meaningless.
//! - Every item runs one untimed warm-up round, then `K = 5` timed rounds;
//!   the fastest round wins (min-of-K) and its wall time is reported.
//! - Query items run `1e5` iterations per round; intern items build a `1e5`
//!   type population per round. `1 M ops/s = 10^6 ops/s`.
//! - Determinism: the population generator is seeded `SplitMix64` and nothing
//!   iterates a `HashMap`, so output is reproducible up to machine noise.
//! - Footprint: a counting global allocator reports net live bytes
//!   (allocations minus deallocations) across table construction; see
//!   `footprint` for the exact window.

#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
// Benchmark arithmetic casts sizes to f64 / smaller ints throughout.
#![allow(clippy::cast_precision_loss)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Instant;

use dartiq_types::{
    CoreDecls, NamedMember, TypeDeclId, TypeHierarchy, TypeId, TypeKind, TypeParamId, TypeSystem,
    TypeTable, UninternedFunction, UninternedType,
};

/// Timed rounds per item; the fastest round determines the reported number.
const ROUNDS: usize = 5;
/// Population size for the intern items (types interned per round).
const N: usize = 100_000;
/// Iterations per round for each subtype/LUB query item.
const ITERS: usize = 100_000;
/// Number of class declarations in the inheritance chain (walk depth).
const CHAIN_DEPTH: usize = 10;
/// Seed type count prepended to the population id pool (`int`, `num`, `str`).
const SEED_TYPES: usize = 3;
/// Generator seed (`SplitMix64`); fixed for reproducibility.
const SEED: u64 = 0x5EED_D1A7_BA5E_0001;
/// How far back the generator samples for nested type arguments.
const RECENT_WINDOW: u64 = 64;

// ---- counting allocator (footprint section) ----

/// Net live bytes (allocated minus deallocated) across the process.
static LIVE_BYTES: AtomicI64 = AtomicI64::new(0);

/// Pass-through wrapper around [`System`] that tracks net live bytes.
struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as i64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size() as i64, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(new_size as i64 - layout.size() as i64, Ordering::Relaxed);
        }
        ptr
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn live_bytes() -> i64 {
    LIVE_BYTES.load(Ordering::Relaxed)
}

// ---- deterministic generator ----

/// `SplitMix64`; fixed seed, no external state, fully deterministic.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// Declaration handles the population generator draws from.
struct GenCtx {
    box_decl: TypeDeclId,
    pair_decl: TypeDeclId,
    chain_decls: Vec<TypeDeclId>,
    int_decl: TypeDeclId,
    num_decl: TypeDeclId,
    string_decl: TypeDeclId,
}

/// Seeds the id pool with primitive interface instances of `table`.
fn seed_pool(table: &TypeTable, ctx: &GenCtx) -> Vec<TypeId> {
    let inst = |decl| {
        table.intern(
            UninternedType::Interface {
                decl,
                args: Vec::new(),
            },
            false,
        )
    };
    vec![
        inst(ctx.num_decl),
        inst(ctx.int_decl),
        inst(ctx.string_decl),
    ]
}

/// Draws one uninterned type: mostly interface instantiations over recent ids
/// (nested `Box`/`Pair` argument combinations), plus `FutureOr`, type
/// parameters, and zero-arg chain classes.
fn gen_type(rng: &mut Rng, ctx: &GenCtx, pool: &[TypeId]) -> UninternedType {
    match rng.below(100) {
        0..=7 => UninternedType::FutureOr {
            base: sample(rng, pool),
        },
        8..=11 => UninternedType::TypeParameter {
            parameter: TypeParamId::from_raw(rng.below(4) as u32),
        },
        12..=15 => {
            let decl = ctx.chain_decls[rng.below(CHAIN_DEPTH as u64) as usize];
            UninternedType::Interface {
                decl,
                args: Vec::new(),
            }
        }
        16..=52 => UninternedType::Interface {
            decl: ctx.box_decl,
            args: vec![sample(rng, pool)],
        },
        _ => UninternedType::Interface {
            decl: ctx.pair_decl,
            args: vec![sample(rng, pool), sample(rng, pool)],
        },
    }
}

/// Samples an argument id, biased toward recently interned types so the
/// population grows nested structures.
fn sample(rng: &mut Rng, pool: &[TypeId]) -> TypeId {
    let window = (pool.len() as u64).min(RECENT_WINDOW);
    let index = pool.len() - 1 - rng.below(window) as usize;
    pool[index]
}

/// Interns `n` generated types, appending every result id to `pool`
/// (seeded via [`seed_pool`]; capacity must cover the extra `n`).
fn populate(table: &TypeTable, ctx: &GenCtx, pool: &mut Vec<TypeId>, n: usize) {
    let mut rng = Rng::new(SEED);
    for _ in 0..n {
        let ty = gen_type(&mut rng, ctx, pool);
        let nullable = rng.next_u64() & 1 == 0;
        let id = table.intern(ty, nullable);
        pool.push(id);
    }
}

/// Rebuilds the uninterned form of a population type so re-interning it is a
/// guaranteed dedup hit.
fn rebuild(table: &TypeTable, id: TypeId) -> (UninternedType, bool) {
    let ty = table.get(id);
    let input = match ty.kind() {
        TypeKind::Interface { decl, args } => UninternedType::Interface {
            decl,
            args: table.args(args),
        },
        TypeKind::FutureOr { base } => UninternedType::FutureOr { base },
        TypeKind::TypeParameter { parameter } => UninternedType::TypeParameter { parameter },
        TypeKind::Never => UninternedType::Never,
        TypeKind::Dynamic => UninternedType::Dynamic,
        TypeKind::Void => UninternedType::Void,
        TypeKind::Invalid => UninternedType::Invalid,
        TypeKind::Null => UninternedType::Null,
        TypeKind::Function { .. } | TypeKind::Record { .. } => {
            unreachable!("population contains no function/record kinds")
        }
    };
    (input, ty.is_nullable())
}

// ---- world construction ----

/// Pre-interned query operands for the subtype/LUB items.
struct Specimens {
    int: TypeId,
    int_q: TypeId,
    num: TypeId,
    num_q: TypeId,
    never: TypeId,
    /// Chain member instances; `chain[0]` is the deepest subclass.
    chain: Vec<TypeId>,
    left: TypeId,
    right: TypeId,
    common: TypeId,
    /// `int Function({required num a, int b})`.
    fn_named_num: TypeId,
    /// `int Function({required int a, int b})`.
    fn_named_int: TypeId,
    futureor_int: TypeId,
    futureor_num: TypeId,
    box_int: TypeId,
    box_num: TypeId,
}

/// Builds the hierarchy snapshot (core decls + a depth-10 chain + generic
/// argument classes), the generator context, and every query operand.
fn build_world(table: &TypeTable) -> (TypeHierarchy, GenCtx, Specimens) {
    let ctx = GenCtx {
        box_decl: TypeDeclId::from_raw(9),
        pair_decl: TypeDeclId::from_raw(10),
        chain_decls: (0..CHAIN_DEPTH as u32)
            .map(|i| TypeDeclId::from_raw(i + 11))
            .collect(),
        int_decl: TypeDeclId::from_raw(4),
        num_decl: TypeDeclId::from_raw(3),
        string_decl: TypeDeclId::from_raw(5),
    };
    let hierarchy = build_hierarchy(table, &ctx);
    let specimens = build_specimens(table, &ctx);
    (hierarchy, ctx, specimens)
}

/// Builds the subtype/LUB hierarchy: the `Object`/`Future`/`Function` cores,
/// an `int <: num` edge, sibling classes sharing `Common`, and the depth-10
/// chain `chain[0] <: chain[1] <: ... <: Object`.
fn build_hierarchy(table: &TypeTable, ctx: &GenCtx) -> TypeHierarchy {
    let object = TypeDeclId::from_raw(0);
    let future = TypeDeclId::from_raw(1);
    let function = TypeDeclId::from_raw(2);
    let common_decl = TypeDeclId::from_raw(6);
    let left_decl = TypeDeclId::from_raw(7);
    let right_decl = TypeDeclId::from_raw(8);
    let interface = |decl| {
        let ty = UninternedType::Interface {
            decl,
            args: Vec::new(),
        };
        table.intern(ty, false)
    };
    let num = interface(ctx.num_decl);
    let chain: Vec<TypeId> = ctx
        .chain_decls
        .iter()
        .map(|&decl| interface(decl))
        .collect();
    let common = interface(common_decl);

    let mut builder = TypeHierarchy::builder(CoreDecls {
        object,
        future,
        function,
        record: None,
    })
    .decl(object, Vec::new())
    .decl(future, vec![TypeParamId::from_raw(0)])
    .decl(function, Vec::new())
    .decl(ctx.num_decl, Vec::new())
    .decl(ctx.int_decl, Vec::new())
    .decl(ctx.string_decl, Vec::new())
    .decl(common_decl, Vec::new())
    .decl(left_decl, Vec::new())
    .decl(right_decl, Vec::new())
    .decl(ctx.box_decl, vec![TypeParamId::from_raw(1)])
    .decl(
        ctx.pair_decl,
        vec![TypeParamId::from_raw(2), TypeParamId::from_raw(3)],
    );
    for &decl in &ctx.chain_decls {
        builder = builder.decl(decl, Vec::new());
    }
    builder = builder.supertype(ctx.int_decl, num);
    builder = builder
        .supertype(left_decl, common)
        .supertype(right_decl, common);
    for (i, &decl) in ctx.chain_decls.iter().enumerate().take(CHAIN_DEPTH - 1) {
        builder = builder.supertype(decl, chain[i + 1]);
    }
    builder
        .build(table)
        .expect("benchmark hierarchy is well-formed")
}

/// Interns every query operand: primitives with both projections, chain
/// members, the sibling pair, the named-parameter function pair, and
/// `FutureOr` / `Box` compositions.
fn build_specimens(table: &TypeTable, ctx: &GenCtx) -> Specimens {
    let common_decl = TypeDeclId::from_raw(6);
    let left_decl = TypeDeclId::from_raw(7);
    let right_decl = TypeDeclId::from_raw(8);
    let interface = |decl, nullable: bool| {
        let ty = UninternedType::Interface {
            decl,
            args: Vec::new(),
        };
        table.intern(ty, nullable)
    };
    let num = interface(ctx.num_decl, false);
    let int = interface(ctx.int_decl, false);
    let chain: Vec<TypeId> = ctx
        .chain_decls
        .iter()
        .map(|&decl| interface(decl, false))
        .collect();
    let common = interface(common_decl, false);
    let member = |name: &str, ty: TypeId, required: bool| NamedMember {
        name: name.to_owned(),
        ty,
        required,
    };
    // `int Function({required <param> a, int b})`.
    let fn_type = |param: TypeId| {
        table
            .intern_function(
                UninternedFunction {
                    return_type: int,
                    type_parameters: Vec::new(),
                    positional: Vec::new(),
                    named: vec![member("a", param, true), member("b", int, false)],
                    required_positional: 0,
                },
                false,
            )
            .expect("well-formed signature")
    };
    let box_of = |arg: TypeId| {
        let ty = UninternedType::Interface {
            decl: ctx.box_decl,
            args: vec![arg],
        };
        table.intern(ty, false)
    };
    Specimens {
        int_q: interface(ctx.int_decl, true),
        num_q: interface(ctx.num_decl, true),
        int,
        num,
        never: table.intern(UninternedType::Never, false),
        chain,
        left: interface(left_decl, false),
        right: interface(right_decl, false),
        common,
        fn_named_num: fn_type(num),
        fn_named_int: fn_type(int),
        futureor_int: table.intern(UninternedType::FutureOr { base: int }, false),
        futureor_num: table.intern(UninternedType::FutureOr { base: num }, false),
        box_int: box_of(int),
        box_num: box_of(num),
    }
}

/// Asserts every benched relation once, so the numbers measure the intended
/// query and not a misuse of the API.
fn check_semantics(ts: &TypeSystem<'_>, s: &Specimens) {
    let deep = CHAIN_DEPTH - 1;
    assert!(ts.is_subtype(s.chain[0], s.chain[deep]), "chain walk");
    assert!(
        !ts.is_subtype(s.chain[deep], s.chain[0]),
        "chain is one-directional"
    );
    assert!(ts.is_subtype(s.int, s.num), "declared int <: num edge");
    assert!(
        ts.is_subtype(s.fn_named_num, s.fn_named_int),
        "named-param contravariance"
    );
    assert!(
        ts.is_subtype(s.futureor_int, s.futureor_num),
        "FutureOr covariance"
    );
    assert!(ts.is_subtype(s.never, s.chain[0]), "Never is bottom");
    assert!(ts.is_subtype(s.int, s.int_q), "int <: int?");
    assert!(
        !ts.is_subtype(s.int_q, s.int),
        "int? <: int must be rejected"
    );
    assert_eq!(
        ts.least_upper_bound(s.left, s.right),
        s.common,
        "sibling LUB"
    );
    assert_eq!(
        ts.least_upper_bound(s.int_q, s.num),
        s.num_q,
        "nullability merge"
    );
    assert_eq!(
        ts.least_upper_bound(s.futureor_int, s.int),
        s.futureor_int,
        "FutureOr merge"
    );
    assert_eq!(
        ts.least_upper_bound(s.box_int, s.box_num),
        s.box_num,
        "pointwise arg merge"
    );
    println!("sanity: semantic preconditions hold\n");
}

// ---- timing harness ----

/// Runs one untimed warm-up, then `ROUNDS` timed rounds of `run` on fresh
/// `setup` state; the fastest round is reported as `ms` and `M ops/s`.
fn bench_min<S>(label: &str, iters: usize, mut setup: impl FnMut() -> S, mut run: impl FnMut(S)) {
    run(setup());
    let mut min = f64::INFINITY;
    for _ in 0..ROUNDS {
        let state = setup();
        let start = Instant::now();
        run(state);
        let secs = start.elapsed().as_secs_f64();
        if secs < min {
            min = secs;
        }
    }
    let ms = min * 1e3;
    let m_ops = iters as f64 / min / 1e6;
    println!("  {label:<46} {ms:>9.3} ms  {m_ops:>10.2} M ops/s");
}

fn bench_query(ts: &TypeSystem<'_>, label: &str, a: TypeId, b: TypeId) {
    bench_min(
        label,
        ITERS,
        || (),
        |()| {
            for _ in 0..ITERS {
                black_box(ts.is_subtype(a, b));
            }
        },
    );
}

fn bench_lub(ts: &TypeSystem<'_>, label: &str, a: TypeId, b: TypeId) {
    bench_min(
        label,
        ITERS,
        || (),
        |()| {
            for _ in 0..ITERS {
                black_box(ts.least_upper_bound(a, b));
            }
        },
    );
}

// ---- sections ----

fn bench_interning(ctx: &GenCtx) {
    println!("== 1. interning (population N = {N}, incl. generator cost) ==");
    let distinct = Cell::new(0usize);
    bench_min(
        "intern, fresh population",
        N,
        || {
            let table = TypeTable::new();
            let mut pool = seed_pool(&table, ctx);
            pool.reserve(N);
            (table, pool)
        },
        |(table, mut pool)| {
            populate(&table, ctx, &mut pool, N);
            distinct.set(table.len());
        },
    );
    let distinct = distinct.get();
    let unique = distinct as f64 / N as f64 * 100.0;
    println!("      {distinct} distinct of {N} interned ({unique:.1}% unique; rest were hits)");

    bench_min(
        "intern, dedup-hit replay (pure intern)",
        N,
        || {
            let table = TypeTable::new();
            let mut pool = seed_pool(&table, ctx);
            pool.reserve(N);
            populate(&table, ctx, &mut pool, N);
            let inputs: Vec<(UninternedType, bool)> = pool[SEED_TYPES..]
                .iter()
                .map(|&id| rebuild(&table, id))
                .collect();
            (table, inputs)
        },
        |(table, inputs)| {
            for (input, nullable) in inputs {
                black_box(table.intern(input, nullable));
            }
        },
    );
    println!();
}

fn bench_subtypes(ts: &TypeSystem<'_>, s: &Specimens) {
    println!("== 2. subtype throughput ({ITERS} queries per item) ==");
    let deep = CHAIN_DEPTH - 1;
    bench_query(
        ts,
        "interface chain, depth-10 walk",
        s.chain[0],
        s.chain[deep],
    );
    bench_query(
        ts,
        "function, named params (contravariant)",
        s.fn_named_num,
        s.fn_named_int,
    );
    bench_query(
        ts,
        "FutureOr<int> <: FutureOr<num>",
        s.futureor_int,
        s.futureor_num,
    );
    bench_query(ts, "Never <: interface (bottom)", s.never, s.chain[0]);
    bench_query(ts, "nullability: int <: int?", s.int, s.int_q);
    bench_query(ts, "nullability: int? <: int (rejected)", s.int_q, s.int);
    bench_query(ts, "nullability: int? <: int?", s.int_q, s.int_q);
    println!();
}

fn bench_lubs(ts: &TypeSystem<'_>, s: &Specimens) {
    println!("== 3. least-upper-bound throughput ({ITERS} queries per item) ==");
    bench_lub(ts, "siblings, supertype-set intersection", s.left, s.right);
    bench_lub(ts, "nullability merge (int? with num)", s.int_q, s.num);
    bench_lub(
        ts,
        "FutureOr merge (FutureOr<int>, int)",
        s.futureor_int,
        s.int,
    );
    bench_lub(
        ts,
        "pointwise args (Box<int>, Box<num>)",
        s.box_int,
        s.box_num,
    );
    println!();
}

/// Builds a fresh table of `n` population types inside a live-bytes window.
///
/// Window = net live bytes (allocations minus deallocations) between the two
/// counter snapshots: it covers the interner `HashMap`, all append-only pools
/// (including capacity slack retained by dedup rollbacks), and transient
/// growth reallocation. The id pool is pre-reserved before the window, so the
/// numbers exclude benchmark scaffolding.
fn footprint(ctx: &GenCtx, n: usize) {
    let table = TypeTable::new();
    let mut pool = seed_pool(&table, ctx);
    pool.reserve(n);
    let before = live_bytes();
    populate(&table, ctx, &mut pool, n);
    let after = live_bytes();
    let distinct = table.len();
    drop(table);
    let residual = live_bytes() - before;
    let delta = after - before;
    let per_type = delta as f64 / distinct as f64;
    println!(
        "  {n:>7} requested, {distinct:>7} distinct: {delta:>11} live bytes  ({per_type:6.1} bytes/type; residual after drop {residual} B)"
    );
}

fn main() {
    println!("dartiq-types microbenchmark");
    println!(
        "method: release build; min of {ROUNDS} timed rounds after 1 warm-up; \
         {ITERS} iters per query item; 1 M ops/s = 10^6 ops/s"
    );
    println!();
    let table = TypeTable::new();
    let (hierarchy, ctx, specimens) = build_world(&table);
    let ts = TypeSystem::new(&table, &hierarchy);
    check_semantics(&ts, &specimens);
    bench_interning(&ctx);
    bench_subtypes(&ts, &specimens);
    bench_lubs(&ts, &specimens);
    println!(
        "== 4. resident footprint (counting allocator, net live bytes across construction) =="
    );
    footprint(&ctx, 10_000);
    footprint(&ctx, N);
}
