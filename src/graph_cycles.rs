use std::collections::HashSet;

use petgraph::{
    graph::{Graph, NodeIndex},
    stable_graph::IndexType,
    EdgeType,
};

/// Minimum and maximum number of distinct vertices in a reported cycle.
const MIN_LEN: usize = 3;
const MAX_LEN: usize = 5;

/// Trait for enumerating cycles in a graph.
pub trait Cycles {
    /// The node identifier of the underlying graph.
    type NodeId;

    /// Find every cycle whose length (number of distinct vertices) is between
    /// [`MIN_LEN`] and [`MAX_LEN`] inclusive.
    ///
    /// Each returned element is the cycle's vertices in traversal order with the
    /// start vertex appended again to close the loop, e.g. `[a, b, c, a]`. Both
    /// traversal directions of an undirected cycle are reported (they are
    /// distinct trades), but rotations of the same directed cycle are reported
    /// only once.
    fn cycles(&self) -> Vec<Vec<Self::NodeId>>;
}

impl<N, E, Ty: EdgeType, Ix: IndexType> Cycles for Graph<N, E, Ty, Ix> {
    type NodeId = NodeIndex<Ix>;

    fn cycles(&self) -> Vec<Vec<Self::NodeId>> {
        let mut result = Vec::new();
        // Dedup is defensive: the min-start construction below already yields
        // each directed cycle once, but a canonical key guards against repeats
        // from parallel edges.
        let mut seen: HashSet<Vec<NodeIndex<Ix>>> = HashSet::new();
        let mut path: Vec<NodeIndex<Ix>> = Vec::with_capacity(MAX_LEN);

        for start in self.node_indices() {
            path.clear();
            path.push(start);
            dfs(self, start, start, &mut path, &mut result, &mut seen);
        }

        result
    }
}

/// Depth-limited DFS that enumerates simple cycles returning to `start`.
///
/// Determinism and dedup come from a single rule: every intermediate vertex
/// must have a strictly greater index than `start`. That forces `start` to be
/// the minimum vertex of any cycle we record, so each directed cycle is found
/// exactly once (in its canonical, min-rooted rotation) and the traversal order
/// is fully determined by the graph's structure — no hashing-order dependence.
fn dfs<N, E, Ty: EdgeType, Ix: IndexType>(
    graph: &Graph<N, E, Ty, Ix>,
    start: NodeIndex<Ix>,
    current: NodeIndex<Ix>,
    path: &mut Vec<NodeIndex<Ix>>,
    result: &mut Vec<Vec<NodeIndex<Ix>>>,
    seen: &mut HashSet<Vec<NodeIndex<Ix>>>,
) {
    for next in graph.neighbors(current) {
        if next == start {
            // Closing the loop back to the start vertex.
            if path.len() >= MIN_LEN && seen.insert(path.clone()) {
                let mut closed = path.clone();
                closed.push(start);
                result.push(closed);
            }
        } else if next > start && path.len() < MAX_LEN && !path.contains(&next) {
            path.push(next);
            dfs(graph, start, next, path, result, seen);
            path.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petgraph::graph::UnGraph;

    fn ring(n: u32) -> UnGraph<(), ()> {
        let edges: Vec<(u32, u32)> = (0..n).map(|i| (i, (i + 1) % n)).collect();
        UnGraph::from_edges(&edges)
    }

    #[test]
    fn triangle_has_both_directions() {
        // A single triangle is one undirected cycle = two directed cycles.
        assert_eq!(ring(3).cycles().len(), 2);
    }

    #[test]
    fn square_and_pentagon_within_bounds() {
        assert_eq!(ring(4).cycles().len(), 2); // length 4
        assert_eq!(ring(5).cycles().len(), 2); // length 5 == MAX_LEN
    }

    #[test]
    fn cycles_over_max_len_are_excluded() {
        // A 6-ring's only cycle has 6 vertices (> MAX_LEN), so nothing is found.
        assert_eq!(ring(6).cycles().len(), 0);
    }

    #[test]
    fn complete_graph_k4_counts() {
        // K4: C(4,3)=4 triangles and 3 four-cycles, each in both directions
        // => 4*2 + 3*2 = 14.
        let k4 = UnGraph::<(), ()>::from_edges(&[
            (0, 1),
            (0, 2),
            (0, 3),
            (1, 2),
            (1, 3),
            (2, 3),
        ]);
        assert_eq!(k4.cycles().len(), 14);
    }

    #[test]
    fn output_is_deterministic() {
        // The whole point of the rewrite: identical results across runs,
        // including ordering (no hashing-order dependence).
        let k4 = UnGraph::<(), ()>::from_edges(&[
            (0, 1),
            (0, 2),
            (0, 3),
            (1, 2),
            (1, 3),
            (2, 3),
        ]);
        assert_eq!(k4.cycles(), k4.cycles());
    }

    #[test]
    fn cycles_are_closed_and_canonical() {
        for cycle in ring(4).cycles() {
            // Closed: first == last.
            assert_eq!(cycle.first(), cycle.last());
            // Canonical: the start (== the repeated endpoint) is the minimum
            // vertex of the cycle.
            let min = cycle.iter().min().unwrap();
            assert_eq!(cycle.first().unwrap(), min);
        }
    }
}
