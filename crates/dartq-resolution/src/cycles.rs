//! Cycle-bounded graph walks (module `cycles.rs`): the shared primitives
//! behind the element-graph recursion diagnostics, pinned to the analyzer
//! oracle (dart 3.13.7).
//!
//! Three faces, one termination invariant: every walk in this module is
//! bounded by a visited set — each node enters the frame stack at most
//! once and each edge is consumed at most once, for `O(V + E)` total work
//! on any input, cyclic or not. No walk can hang, and the oracle's own
//! cycle-breaking postures mirror through:
//!
//! - [`cycle_through`] — the `recursive_constructor_redirect` face
//!   (`error_verifier.dart:8102-8111`
//!   `_hasRedirectingFactoryConstructorCycle`): follow successors with a
//!   visited set; the answer is positive only when the walk returns to
//!   **the starting node** — a walk that merely enters a cycle elsewhere
//!   stays silent (`identical(current, constructor)`).
//! - [`dependency_cycle`] — the `top_level_cycle` /
//!   `recursive_compile_time_constant` face
//!   (`top_level_inference.dart:268-284`; `constant/evaluation.dart:433-459`
//!   `generateCycleError`): a depth-first walk in caller edge order that,
//!   on the first back edge into the current path, returns the path slice
//!   from the revisited node — exactly the oracle's on-inferring-stack
//!   slice `_inferring.slice(startIndex)`. Every member of that slice
//!   carries the error upstream (`resolver.dart:4552-4572`
//!   `_checkTopLevelCycle` sorts the names for `{1}`;
//!   `recursive_compile_time_constant` reports at each variable of the
//!   cycle).
//! - [`strongly_connected`] — the `recursive_interface_inheritance` face
//!   (`summary2/interface_cycles.dart` `breakInterfaceCycles` over
//!   `_fe_analyzer_shared/src/util/dependency_walker.dart`, Tarjan's
//!   algorithm): the cyclic strongly connected component of a node —
//!   more than one member, or a single member with a self-edge — in the
//!   oracle's pop order (reverse discovery within the component, the
//!   order `evaluateScc` receives and `interfaceCycle` stores; the loop
//!   message joins those display names with `', '`,
//!   `inheritance_override.dart:776-780`). Upstream also **breaks** the
//!   cycle after flagging it (supertype reset to `Object`, mixins and
//!   interfaces cleared, `interface_cycles.dart:76-93`) — that reset is
//!   the element model's business; the face's contract is that the walk
//!   terminates and answers on any cyclic shape.
//!
//! The primitives are node-generic: the semantic layer feeds its own node
//! identity (element ids, constructor ids, declaration names) and edge
//! order; behavior is a total function of that input.

use rustc_hash::{FxHashMap, FxHashSet};

/// Whether the successor walk from `start` returns to `start`, and the
/// cycle's member path.
///
/// Mirrors `_hasRedirectingFactoryConstructorCycle`
/// (`error_verifier.dart:8102-8111`): the walk follows successors in
/// caller order with a visited set; only re-entering **`start`** answers
/// `Some` — a cycle the walk passes *into* (not through) stays silent,
/// matching the oracle's `identical(current, constructor)` gate. The
/// returned path lists the cycle's members starting at `start`; the last
/// member's edge back to `start` closes the loop (a self-loop is the
/// single-member path `[start]`).
///
/// Bounded: each node is pushed at most once (unvisited at push time),
/// each edge consumed at most once.
pub fn cycle_through<N>(start: N, successors: impl Fn(N) -> Vec<N>) -> Option<Vec<N>>
where
    N: Copy + Eq + std::hash::Hash,
{
    let mut path = vec![start];
    let mut on_path = FxHashSet::from_iter([start]);
    let mut exhausted: FxHashSet<N> = FxHashSet::default();
    let mut frames: Vec<(N, std::vec::IntoIter<N>)> = vec![(start, successors(start).into_iter())];
    loop {
        let Some(frame) = frames.last_mut() else {
            return None;
        };
        let node = frame.0;
        match frame.1.next() {
            // The frame is spent: its subtree cannot reach `start`, so the
            // node leaves the current path too.
            None => {
                frames.pop();
                path.pop();
                on_path.remove(&node);
                exhausted.insert(node);
            }
            Some(next) => {
                if next == start {
                    return Some(path);
                }
                // A back edge into the current path (not through `start`)
                // is a different cycle: the oracle's walk would loop there
                // without ever re-entering `start`, so it is not our
                // answer; nodes known exhausted cannot lead back either.
                if on_path.contains(&next) || exhausted.contains(&next) {
                    continue;
                }
                on_path.insert(next);
                path.push(next);
                frames.push((next, successors(next).into_iter()));
            }
        }
    }
}

/// The first cycle the depth-first walk from `start` meets, as the on-path
/// slice from the revisited node — the `top_level_cycle` /
/// `recursive_compile_time_constant` cycle face.
///
/// Mirrors the oracle's on-inferring-stack slice
/// (`top_level_inference.dart:268-284`: the re-requested element is on
/// `_inferring`, the cycle is `_inferring.slice(startIndex)`, and every
/// slice member carries the shared error): a gray/black depth-first walk
/// in caller edge order; the first back edge into the current path
/// returns `path[at..]` — the members of that cycle in walk order, no
/// repetition. `None` when the walk from `start` is acyclic.
///
/// The slice's members — not `start` — are the reporting set: a walk that
/// passes *into* a cycle reports that cycle's members, exactly as the
/// oracle marks them. `top_level_cycle` renders `{1}` from the members'
/// names **sorted** (`error.cycle.map(...).sorted()`); sorting is the
/// caller's render step. Bounded: each node grays once, each edge is
/// consumed once.
pub fn dependency_cycle<N>(start: N, successors: impl Fn(N) -> Vec<N>) -> Option<Vec<N>>
where
    N: Copy + Eq + std::hash::Hash,
{
    // 1 = gray (on the current path), 2 = black (walk exhausted).
    let mut color: FxHashMap<N, u8> = FxHashMap::default();
    let mut path: Vec<N> = Vec::new();
    let mut frames: Vec<(N, std::vec::IntoIter<N>)> = vec![(start, successors(start).into_iter())];
    color.insert(start, 1);
    path.push(start);
    loop {
        let Some(frame) = frames.last_mut() else {
            return None;
        };
        let node = frame.0;
        match frame.1.next() {
            None => {
                frames.pop();
                path.pop();
                color.insert(node, 2);
            }
            Some(next) => match color.get(&next) {
                None => {
                    color.insert(next, 1);
                    path.push(next);
                    frames.push((next, successors(next).into_iter()));
                }
                // Back edge into the current path: the cycle is the path
                // slice from the revisited node. Gray nodes are exactly
                // the path members, so the position always exists.
                Some(1) => {
                    let at = path
                        .iter()
                        .position(|member| *member == next)
                        .expect("a gray node is on the current path");
                    return Some(path[at..].to_vec());
                }
                Some(_) => {}
            },
        }
    }
}

/// The cyclic strongly connected components of a node set, in the oracle's
/// Tarjan pop order — the `recursive_interface_inheritance` face.
///
/// Built by [`strongly_connected`] over `nodes` in caller order (the
/// oracle walks declarations in order, `interface_cycles.dart:23-36`) and
/// a successor closure (the interface edges: supertype, mixins,
/// interfaces, and a mixin's superclass constraints,
/// `interface_cycles.dart:52-60`).
pub struct StronglyConnected<N> {
    /// Node → its component, for cyclic components only (acyclic nodes
    /// answer `None` by absence).
    component_of: FxHashMap<N, u32>,
    /// The cyclic components, in discovery order, each in Tarjan pop
    /// order (reverse discovery within the component) — the list shape
    /// `evaluateScc` receives and `interfaceCycle` stores.
    components: Vec<Vec<N>>,
}

/// Runs Tarjan's strongly connected components over `nodes` (walked in
/// order, mirroring `breakInterfaceCycles`' declaration sweep) with
/// `successors` as the edge face, collecting the **cyclic** components —
/// more than one member, or a single member with a self-edge (the shared
/// walker's `hasTrivialCycle`, `dependency_walker.dart:72`).
///
/// Iterative (explicit frame stack — the upstream recursion is a
/// stack-overflow risk dartq does not inherit) and bounded: each node is
/// framed once, each edge consumed once. The `index`/`lowLink` discipline
/// and the pop-until-root component extraction mirror
/// `dependency_walker.dart:51-122` so a component's member order is the
/// oracle's `evaluateScc` order exactly.
pub fn strongly_connected<N>(nodes: &[N], successors: impl Fn(N) -> Vec<N>) -> StronglyConnected<N>
where
    N: Copy + Eq + std::hash::Hash,
{
    let mut index: FxHashMap<N, u32> = FxHashMap::default();
    let mut lowlink: FxHashMap<N, u32> = FxHashMap::default();
    let mut on_tarjan_stack: FxHashSet<N> = FxHashSet::default();
    let mut tarjan_stack: Vec<N> = Vec::new();
    let mut self_loops: FxHashSet<N> = FxHashSet::default();
    let mut component_of: FxHashMap<N, u32> = FxHashMap::default();
    let mut components: Vec<Vec<N>> = Vec::new();
    let mut next_index = 1_u32;

    for &root in nodes {
        if index.contains_key(&root) {
            continue;
        }
        // strongConnect(root), iterative: each frame carries its node and
        // the remaining dependencies; entering assigns the fresh index.
        index.insert(root, next_index);
        lowlink.insert(root, next_index);
        next_index += 1;
        tarjan_stack.push(root);
        on_tarjan_stack.insert(root);
        let mut frames: Vec<(N, std::vec::IntoIter<N>)> =
            vec![(root, successors(root).into_iter())];
        loop {
            let Some(frame) = frames.last_mut() else {
                break;
            };
            let node = frame.0;
            match frame.1.next() {
                None => {
                    frames.pop();
                    // Leaving the frame: merge the child's lowlink into
                    // the parent's (`dependency_walker.dart:82-84`), then
                    // close the component at its root.
                    if let Some(parent) = frames.last_mut() {
                        let merged = lowlink[&parent.0].min(lowlink[&node]);
                        lowlink.insert(parent.0, merged);
                    }
                    if lowlink[&node] == index[&node] {
                        let mut component = Vec::new();
                        loop {
                            let member =
                                tarjan_stack.pop().expect("the root is on the Tarjan stack");
                            on_tarjan_stack.remove(&member);
                            component.push(member);
                            if member == node {
                                break;
                            }
                        }
                        // Cyclic iff multi-member or self-looping;
                        // singleton acyclic components are never stored.
                        if component.len() > 1 || self_loops.contains(&node) {
                            let id = u32::try_from(components.len())
                                .expect("more than u32::MAX components");
                            for member in &component {
                                component_of.insert(*member, id);
                            }
                            components.push(component);
                        }
                    }
                }
                Some(dep) => {
                    if !index.contains_key(&dep) {
                        // Fresh dependency: recurse (assign index, push).
                        index.insert(dep, next_index);
                        lowlink.insert(dep, next_index);
                        next_index += 1;
                        tarjan_stack.push(dep);
                        on_tarjan_stack.insert(dep);
                        frames.push((dep, successors(dep).into_iter()));
                    } else if on_tarjan_stack.contains(&dep) {
                        if dep == node {
                            self_loops.insert(node);
                        }
                        // Back edge into the open component.
                        let merged = lowlink[&node].min(index[&dep]);
                        lowlink.insert(node, merged);
                    }
                    // A visited, popped dependency belongs to a closed
                    // component: skipped (`dependency.isEvaluated`
                    // continue).
                }
            }
        }
    }
    StronglyConnected {
        component_of,
        components,
    }
}

impl<N: Copy + Eq + std::hash::Hash> StronglyConnected<N> {
    /// The cyclic component containing `node` — its members in the
    /// oracle's pop order (the `interfaceCycle` list the loop message
    /// joins), or `None` when `node` sits on no cycle. One hash hit.
    #[must_use]
    pub fn cyclic_component(&self, node: N) -> Option<&[N]> {
        self.component_of
            .get(&node)
            .map(|&id| self.components[id as usize].as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The graph faces take integer nodes and an edge table; `edges`
    /// mirrors the caller-controlled successor order the oracle's walk
    /// orders are pinned to.
    fn walk(edges: &[(u32, &[u32])], node: u32) -> Vec<u32> {
        edges
            .iter()
            .find(|(from, _)| *from == node)
            .map_or(Vec::new(), |(_, to)| (*to).to_vec())
    }

    fn walk_all(edges: &[(u32, &[u32])], node: u32) -> Vec<u32> {
        let mut all: Vec<u32> = edges
            .iter()
            .filter(|(from, _)| *from == node)
            .flat_map(|(_, to)| (*to).iter().copied())
            .collect();
        all.sort_unstable();
        all
    }

    // ---- cycle_through ----

    /// A self-loop answers the single-member path.
    #[test]
    fn cycle_through_self_loop() {
        let edges = [(1_u32, &[1_u32][..])];
        assert_eq!(cycle_through(1, |n| walk(&edges, n)), Some(vec![1]));
    }

    /// A two-step loop answers both members from the start.
    #[test]
    fn cycle_through_two_nodes() {
        let edges = [(1, &[2][..]), (2, &[1][..])];
        assert_eq!(cycle_through(1, |n| walk(&edges, n)), Some(vec![1, 2]));
    }

    /// A walk that enters a cycle not containing the start stays silent —
    /// the oracle's `identical(current, constructor)` gate.
    #[test]
    fn cycle_through_foreign_cycle_is_silent() {
        let edges = [(1, &[2][..]), (2, &[3][..]), (3, &[2][..])];
        assert_eq!(cycle_through(1, |n| walk(&edges, n)), None);
        // The cycle's own member answers from inside.
        assert_eq!(cycle_through(2, |n| walk(&edges, n)), Some(vec![2, 3]));
    }

    /// A diamond with no loop answers `None`, and a long chain terminates
    /// (each node framed once).
    #[test]
    fn cycle_through_acyclic_shapes_answer_none() {
        let diamond = [(1, &[2, 3][..]), (2, &[4][..]), (3, &[4][..])];
        assert_eq!(cycle_through(1, |n| walk(&diamond, n)), None);

        let chain: Vec<(u32, &[u32])> = (0..10_000_u32)
            .map(|n| (n, Box::leak(Box::new([n + 1])) as &[u32]))
            .collect();
        assert_eq!(cycle_through(0, |n| walk(&chain, n)), None);
    }

    /// With several branches, the branch that closes back through the
    /// start answers even after a dead branch is explored first.
    #[test]
    fn cycle_through_picks_the_branch_through_start() {
        let edges = [(1, &[2, 3][..]), (2, &[4][..]), (3, &[1][..])];
        assert_eq!(cycle_through(1, |n| walk(&edges, n)), Some(vec![1, 3]));
    }

    // ---- dependency_cycle ----

    /// A walk that passes into a cycle reports the cycle's own members —
    /// the on-stack slice, not the walk's prefix.
    #[test]
    fn dependency_cycle_reports_the_slice_from_the_revisit() {
        let edges = [(1, &[2][..]), (2, &[3][..]), (3, &[2][..])];
        assert_eq!(dependency_cycle(1, |n| walk(&edges, n)), Some(vec![2, 3]));
    }

    /// A loop back to the start reports the whole path, and the first
    /// back edge in edge order wins when several exist.
    #[test]
    fn dependency_cycle_first_back_edge_wins() {
        let edges = [(1, &[2][..]), (2, &[3][..]), (3, &[1, 2][..])];
        assert_eq!(
            dependency_cycle(1, |n| walk(&edges, n)),
            Some(vec![1, 2, 3])
        );
    }

    /// Acyclic walks answer `None`; `top_level_cycle`'s sorted-name render
    /// is the caller's step, not the walk's.
    #[test]
    fn dependency_cycle_acyclic_answers_none() {
        let edges = [(1, &[2, 3][..]), (2, &[][..]), (3, &[2][..])];
        assert_eq!(dependency_cycle(1, |n| walk(&edges, n)), None);
    }

    // ---- strongly_connected ----

    /// A two-node component lists its members in the oracle's pop order —
    /// reverse discovery — and a node feeding into the cycle from outside
    /// is not a member (its singleton component is not cyclic).
    #[test]
    fn scc_two_node_cycle_pop_order_and_outside_feeder() {
        // Discovery order C, A, B: the component pops [B, A] — the list
        // `evaluateScc` receives and `interfaceCycle` stores.
        let edges = [(3, &[1][..]), (1, &[2][..]), (2, &[1][..])];
        let sccs = strongly_connected(&[3, 1, 2], |n| walk(&edges, n));
        assert_eq!(sccs.cyclic_component(1), Some(&[2_u32, 1][..]));
        assert_eq!(sccs.cyclic_component(2), Some(&[2_u32, 1][..]));
        assert_eq!(sccs.cyclic_component(3), None);
    }

    /// A self-looping singleton is a cyclic component; a plain singleton
    /// is not (the shared walker's `hasTrivialCycle`).
    #[test]
    fn scc_self_loop_singleton_is_cyclic() {
        let edges = [(1, &[1, 2][..]), (2, &[][..])];
        let sccs = strongly_connected(&[1, 2], |n| walk(&edges, n));
        assert_eq!(sccs.cyclic_component(1), Some(&[1_u32][..]));
        assert_eq!(sccs.cyclic_component(2), None);
    }

    /// A four-node strongly connected component pops in reverse discovery
    /// order (trace: A B D C pushed; C D B A popped).
    #[test]
    fn scc_full_component_pop_order() {
        // A → B → D → A with A → C → D: one component of four.
        let edges = [
            (1_u32, &[2_u32, 3][..]),
            (2, &[4][..]),
            (3, &[4][..]),
            (4, &[1][..]),
        ];
        let sccs = strong_connected_sorted(&edges);
        assert_eq!(sccs.cyclic_component(1), Some(&[3_u32, 4, 2, 1][..]));
    }

    /// Disjoint cycles index independently and every member answers its
    /// own component.
    #[test]
    fn scc_disjoint_cycles() {
        let edges = [
            (1_u32, &[2][..]),
            (2, &[1][..]),
            (5, &[6][..]),
            (6, &[5][..]),
        ];
        let sccs = strong_connected_sorted(&edges);
        assert_eq!(sccs.cyclic_component(1), Some(&[2_u32, 1][..]));
        // Discovery 5 then 6 pops 6 first — reverse discovery order.
        assert_eq!(sccs.cyclic_component(6), Some(&[6_u32, 5][..]));
    }

    /// The walk covers nodes reachable only through edges, and terminates
    /// on a mixed graph with a component behind a tail chain plus a
    /// separate component — the interface graph's shape.
    #[test]
    fn scc_discovers_through_edges_and_terminates_on_mixed_shapes() {
        let edges = [
            (7_u32, &[8][..]),
            (8, &[9][..]),
            (9, &[8, 10][..]),
            (10, &[11][..]),
            (11, &[10][..]),
        ];
        let sccs = strong_connected_sorted(&edges);
        assert_eq!(sccs.cyclic_component(8), Some(&[9_u32, 8][..]));
        assert_eq!(sccs.cyclic_component(10), Some(&[11_u32, 10][..]));
        assert_eq!(sccs.cyclic_component(11), Some(&[11_u32, 10][..]));
        // The tail node feeds the first component but joins none.
        assert_eq!(sccs.cyclic_component(7), None);
    }

    /// A helper asserting stable behavior over the edge table: nodes
    /// sorted as the root order, edges walked in table order.
    fn strong_connected_sorted(edges: &[(u32, &[u32])]) -> StronglyConnected<u32> {
        let mut nodes: Vec<u32> = edges.iter().map(|(from, _)| *from).collect();
        nodes.sort_unstable();
        nodes.dedup();
        strongly_connected(&nodes, |n| walk_all(edges, n))
    }
}
