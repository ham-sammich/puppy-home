//! GitKraken-style commit-graph **layout** (the pure topology half).
//!
//! This module is deliberately render-free: it turns a list of commits into
//! lane assignments + edges that the `git_graph_view` module paints. Keeping
//! the algorithm separate means the topology can be unit-tested without a GPU
//! in sight (Zen of Python: if it's hard to test, the design is wrong).

use crate::git::Commit;

/// Which slice of a row's vertical cell an edge occupies.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EdgeHalf {
    /// Top of the cell down to the node: a line converging into this commit.
    Top,
    /// The node down to the bottom: a parent line diverging from this commit.
    Bottom,
    /// Top all the way to bottom: a line passing this row untouched.
    Full,
}

/// One drawn segment within a row's cell.
#[derive(Clone, Debug)]
pub(crate) struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub color: usize,
    pub half: EdgeHalf,
}

/// A single commit laid out: its node lane/color and the edges in its cell.
#[derive(Clone)]
pub(crate) struct GraphRow {
    pub commit: Commit,
    pub node_lane: usize,
    pub node_color: usize,
    pub edges: Vec<GraphEdge>,
}

/// The full laid-out graph: rows top-to-bottom + the widest lane count seen.
pub(crate) struct GraphLayout {
    pub rows: Vec<GraphRow>,
    pub lanes: usize,
}

/// Reuse the first free lane (a `None` hole) or grow a new one.
fn free_lane(lanes: &mut Vec<Option<String>>, lane_color: &mut Vec<usize>) -> usize {
    if let Some(i) = lanes.iter().position(Option::is_none) {
        i
    } else {
        lanes.push(None);
        lane_color.push(0);
        lanes.len() - 1
    }
}

/// Assign every commit to a lane and compute the edges that connect them.
///
/// `commits` must be newest-first with parent hashes populated (as produced by
/// [`crate::git::graph_log`]). Children always precede their parents.
pub(crate) fn compute_graph(commits: &[Commit]) -> GraphLayout {
    // Each active lane holds the hash of the *next* commit expected in it.
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut lane_col: Vec<usize> = Vec::new();
    let mut next_color = 0usize;
    let mut rows = Vec::with_capacity(commits.len());
    let mut max_lanes = 0usize;

    for commit in commits {
        let h = commit.hash.as_str();

        // Lanes currently waiting for this commit (its children's lines).
        let matched: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_deref() == Some(h))
            .map(|(i, _)| i)
            .collect();

        // The node's lane: the first matching child line, or a fresh tip.
        let node_lane = match matched.first() {
            Some(&first) => first,
            None => {
                let lane = free_lane(&mut lanes, &mut lane_col);
                lanes[lane] = Some(commit.hash.clone());
                lane_col[lane] = next_color;
                next_color += 1;
                lane
            }
        };
        let node_color = lane_col[node_lane];

        let mut edges = Vec::new();

        // Top half: every child line converges into the node.
        for &idx in &matched {
            edges.push(GraphEdge {
                from: idx,
                to: node_lane,
                color: lane_col[idx],
                half: EdgeHalf::Top,
            });
        }

        // Lines unrelated to this commit pass straight through the cell.
        for (l, slot) in lanes.iter().enumerate() {
            if slot.is_some() && l != node_lane && !matched.contains(&l) {
                edges.push(GraphEdge {
                    from: l,
                    to: l,
                    color: lane_col[l],
                    half: EdgeHalf::Full,
                });
            }
        }

        // Extra child lanes (beyond the node's) collapse — free them.
        for &idx in matched.iter().skip(1) {
            lanes[idx] = None;
        }

        // Bottom half: parents diverge from the node.
        if commit.parents.is_empty() {
            lanes[node_lane] = None; // a root commit: this lane ends here.
        } else {
            // First parent keeps the node's lane + color (the "main" line down).
            lanes[node_lane] = Some(commit.parents[0].clone());
            edges.push(GraphEdge {
                from: node_lane,
                to: node_lane,
                color: node_color,
                half: EdgeHalf::Bottom,
            });
            // Further parents (merges): route to an existing waiting lane, or
            // open a fresh one with a new color.
            for parent in &commit.parents[1..] {
                let target = match lanes
                    .iter()
                    .position(|l| l.as_deref() == Some(parent.as_str()))
                {
                    Some(e) => e,
                    None => {
                        let lane = free_lane(&mut lanes, &mut lane_col);
                        lanes[lane] = Some(parent.clone());
                        lane_col[lane] = next_color;
                        next_color += 1;
                        lane
                    }
                };
                edges.push(GraphEdge {
                    from: node_lane,
                    to: target,
                    color: lane_col[target],
                    half: EdgeHalf::Bottom,
                });
            }
        }

        max_lanes = max_lanes.max(lanes.len());
        rows.push(GraphRow {
            commit: commit.clone(),
            node_lane,
            node_color,
            edges,
        });
    }

    GraphLayout {
        rows,
        lanes: max_lanes.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.into(),
            short: hash.into(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn linear_history_stays_in_one_lane() {
        let commits = vec![c("c", &["b"]), c("b", &["a"]), c("a", &[])];
        let g = compute_graph(&commits);
        assert_eq!(g.lanes, 1);
        assert!(g.rows.iter().all(|r| r.node_lane == 0));
        // Root commit has no continuing line down.
        let root = g.rows.last().unwrap();
        assert!(!root.edges.iter().any(|e| e.half == EdgeHalf::Bottom));
    }

    #[test]
    fn branch_then_merge_uses_two_lanes_then_collapses() {
        // m merges feature(f) and main(b); both descend from a.
        let commits = vec![
            c("m", &["b", "f"]),
            c("f", &["a"]),
            c("b", &["a"]),
            c("a", &[]),
        ];
        let g = compute_graph(&commits);
        assert!(g.lanes >= 2, "a merge should widen the graph");
        // The merge row diverges into two parent lines.
        let merge = &g.rows[0];
        assert_eq!(
            merge
                .edges
                .iter()
                .filter(|e| e.half == EdgeHalf::Bottom)
                .count(),
            2
        );
        // Everything converges back to a single lane at the root.
        assert_eq!(g.rows.last().unwrap().node_lane, 0);
    }

    #[test]
    fn empty_input_is_safe() {
        let g = compute_graph(&[]);
        assert!(g.rows.is_empty());
        assert_eq!(g.lanes, 1);
    }
}
