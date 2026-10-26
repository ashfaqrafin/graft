//! Property tests: random DAGs and random visible subsets must always lay out
//! with children above parents, one commit per cell, and no edge running
//! through a commit.

use std::collections::HashSet;

use proptest::prelude::*;

use super::{Horizontal, Orientation, Vertical, assign, topo_order};
use crate::model::{Graph, NodeId};
use crate::render::draw::edge_path;

/// Up to 24 commits; each has 0-3 parents chosen among older commits.
fn dag() -> impl Strategy<Value = Vec<Vec<NodeId>>> {
    (1usize..24).prop_flat_map(|n| {
        let per_node: Vec<_> = (0..n)
            .map(|i| {
                let older = n - i - 1;
                if older == 0 {
                    Just(Vec::new()).boxed()
                } else {
                    proptest::collection::vec(0..older, 0..=3.min(older))
                        .prop_map(move |picks| {
                            let mut ps: Vec<NodeId> = Vec::new();
                            for p in picks {
                                let id = i + 1 + p;
                                if !ps.contains(&id) {
                                    ps.push(id);
                                }
                            }
                            ps
                        })
                        .boxed()
                }
            })
            .collect();
        per_node
    })
}

fn check(graph: &Graph, visible: &[bool], priority: &[NodeId]) -> Result<(), TestCaseError> {
    let order = topo_order(graph, priority);
    prop_assert_eq!(order.len(), graph.nodes.len(), "topological order is a permutation");
    let pos_in: Vec<usize> = {
        let mut v = vec![0; order.len()];
        for (i, n) in order.iter().enumerate() {
            v[*n] = i;
        }
        v
    };
    for (c, node) in graph.nodes.iter().enumerate() {
        for &p in &node.parents {
            prop_assert!(pos_in[c] < pos_in[p], "child {c} before parent {p}");
        }
    }

    let layout = assign(graph, &order, visible);
    let orients: [Box<dyn Orientation>; 3] = [
        Box::new(Vertical { x0: 2.0, y0: 1.0, lane_w: 2.0, row_h: 2.0 }),
        Box::new(Vertical { x0: 2.0, y0: 1.0, lane_w: 2.0, row_h: 1.0 }),
        Box::new(Horizontal { right: 200.0, y0: 1.0, col_w: 4.0, lane_h: 3.0 }),
    ];
    for (k, o) in orients.iter().enumerate() {
        let horizontal = k == 2;
        let mut cells: HashSet<(i32, i32)> = HashSet::new();
        let mut at = vec![(0i32, 0i32); graph.nodes.len()];
        for n in 0..graph.nodes.len() {
            if !visible[n] {
                prop_assert!(layout.row[n].is_none());
                continue;
            }
            let (x, y) = o.place(layout.lane[n].unwrap(), layout.row[n].unwrap());
            at[n] = (x as i32, y as i32);
            prop_assert!(cells.insert(at[n]), "two commits share a cell");
        }
        let mut path = Vec::new();
        for (e, (c, p)) in graph.edges().into_iter().enumerate() {
            if !(visible[c] && visible[p]) {
                continue;
            }
            let lane = o.lane_coord(layout.edge_lane[e].expect("visible edge has a lane")) as i32;
            if horizontal {
                edge_path(&mut path, (at[c].1, -at[c].0), lane, (at[p].1, -at[p].0));
                for cell in path.iter_mut() {
                    *cell = (-cell.1, cell.0);
                }
            } else {
                edge_path(&mut path, at[c], lane, at[p]);
            }
            for cell in &path {
                prop_assert!(!cells.contains(cell), "edge {c}->{p} runs through a commit at {cell:?}");
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn layout_is_sound(parents in dag(), mask in proptest::collection::vec(any::<bool>(), 24), first in 0usize..24) {
        let graph = Graph::synthetic(&parents);
        let n = graph.nodes.len();
        check(&graph, &vec![true; n], &[first % n])?;
        // Any subset closed under "parent of a visible commit is visible", like a
        // before or after state.
        let mut visible: Vec<bool> = (0..n).map(|i| mask[i]).collect();
        for i in 0..n {
            if visible[i] {
                for &p in &graph.nodes[i].parents {
                    visible[p] = true;
                }
            }
        }
        check(&graph, &visible, &[])?;
    }
}
