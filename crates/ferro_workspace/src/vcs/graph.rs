//! Commit graph layout for the Git log: assigns every commit a lane and
//! computes the line segments drawn in each row.
//!
//! Commits arrive newest first. Each lane "waits" for one commit id (the
//! parent of a commit drawn above). A commit takes the lane waiting for it;
//! other lanes waiting for the same commit converge into it (a fork seen from
//! below). Its first parent continues in the commit's lane; further parents
//! (a merge) get a lane of their own, reusing freed lanes first. Parents not
//! in the list (filtered out, or beyond the loaded page) are ignored, so
//! their lanes end instead of running forever.

use std::collections::HashSet;

use ferro_services::Commit;

/// A line segment within one half of a row, from lane `from` (at the top of
/// that half) to lane `to` (at its bottom).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    /// Index into the graph palette.
    pub color: usize,
}

/// What to draw for one commit row.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GraphRow {
    /// The lane of the commit's dot.
    pub node: usize,
    pub node_color: usize,
    /// Segments from the top of the row to its middle.
    pub upper: Vec<Edge>,
    /// Segments from the middle of the row to its bottom.
    pub lower: Vec<Edge>,
    /// Lanes in use in this row (for the column width).
    pub width: usize,
}

#[derive(Clone)]
struct Lane {
    /// The commit this lane leads to.
    waiting_for: String,
    color: usize,
}

/// Lay out `commits` (newest first).
pub fn layout_graph(commits: &[Commit]) -> Vec<GraphRow> {
    let present: HashSet<&str> = commits.iter().map(|c| c.id.as_str()).collect();
    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut next_color = 0usize;
    let mut new_color = || {
        let c = next_color;
        next_color += 1;
        c
    };
    let mut rows = Vec::with_capacity(commits.len());

    for commit in commits {
        let before = lanes.clone();
        let waiting: Vec<usize> = before
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_ref().is_some_and(|l| l.waiting_for == commit.id))
            .map(|(ix, _)| ix)
            .collect();

        // The node's lane: the first lane waiting for it, else a free lane.
        let (node, node_color) = match waiting.first() {
            Some(&ix) => (ix, before[ix].as_ref().map(|l| l.color).unwrap_or(0)),
            None => {
                let ix = free_lane(&lanes);
                (ix, new_color())
            }
        };
        if node == lanes.len() {
            lanes.push(None);
        }

        // Upper half: lanes pass through, or converge into the node.
        let mut upper = Vec::new();
        for (ix, lane) in before.iter().enumerate() {
            if let Some(lane) = lane {
                let to = if waiting.contains(&ix) { node } else { ix };
                upper.push(Edge {
                    from: ix,
                    to,
                    color: lane.color,
                });
            }
        }
        for &ix in &waiting {
            lanes[ix] = None;
        }

        // Parents.
        let parents: Vec<&String> = commit
            .parents
            .iter()
            .filter(|p| present.contains(p.as_str()))
            .collect();
        let mut parent_edges = Vec::new();
        for (n, parent) in parents.iter().enumerate() {
            let existing = lanes
                .iter()
                .position(|l| l.as_ref().is_some_and(|l| &l.waiting_for == *parent))
                // The first parent stays in the node's lane when that lane
                // is further left: the other lane converges into it later,
                // which keeps first-parent history straight.
                .filter(|&ix| !(n == 0 && ix > node && lanes[node].is_none()));
            match existing {
                Some(ix) => {
                    let color = lanes[ix].as_ref().map(|l| l.color).unwrap_or(0);
                    parent_edges.push(Edge {
                        from: node,
                        to: ix,
                        color,
                    });
                }
                None => {
                    let (ix, color) = if n == 0 && lanes[node].is_none() {
                        (node, node_color)
                    } else {
                        let ix = free_lane(&lanes);
                        if ix == lanes.len() {
                            lanes.push(None);
                        }
                        (ix, new_color())
                    };
                    lanes[ix] = Some(Lane {
                        waiting_for: (*parent).clone(),
                        color,
                    });
                    parent_edges.push(Edge {
                        from: node,
                        to: ix,
                        color,
                    });
                }
            }
        }

        // Lower half: untouched lanes pass through, plus the parent edges.
        let mut lower = Vec::new();
        for (ix, lane) in before.iter().enumerate() {
            if ix == node || waiting.contains(&ix) {
                continue;
            }
            if let Some(lane) = lane {
                lower.push(Edge {
                    from: ix,
                    to: ix,
                    color: lane.color,
                });
            }
        }
        lower.extend(parent_edges);

        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        let width = before
            .len()
            .max(lanes.len())
            .max(node + 1)
            .max(lower.iter().map(|e| e.to + 1).max().unwrap_or(0));
        rows.push(GraphRow {
            node,
            node_color,
            upper,
            lower,
            width,
        });
    }
    rows
}

fn free_lane(lanes: &[Option<Lane>]) -> usize {
    lanes
        .iter()
        .position(Option::is_none)
        .unwrap_or(lanes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: "A".into(),
            timestamp: 0,
            subject: id.into(),
            body: String::new(),
            refs: Vec::new(),
            files: Vec::new(),
        }
    }

    #[test]
    fn linear_history_uses_one_lane() {
        let log = vec![commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])];
        let rows = layout_graph(&log);
        assert!(rows.iter().all(|r| r.node == 0 && r.width == 1));
        assert!(rows[0].upper.is_empty(), "nothing above the newest commit");
        assert_eq!(
            rows[1].upper,
            vec![Edge {
                from: 0,
                to: 0,
                color: 0
            }]
        );
        assert!(rows[2].lower.is_empty(), "the root has no parents");
    }

    #[test]
    fn a_merge_opens_a_lane_that_joins_back() {
        // m merges f (feature) into b (main); both come from a.
        let log = vec![
            commit("m", &["b", "f"]),
            commit("f", &["a"]),
            commit("b", &["a"]),
            commit("a", &[]),
        ];
        let rows = layout_graph(&log);
        // The merge forks into two lanes.
        assert_eq!(rows[0].node, 0);
        assert_eq!(rows[0].lower.len(), 2);
        assert_eq!(rows[0].lower[1].to, 1);
        // f sits in lane 1, b in lane 0.
        assert_eq!(rows[1].node, 1);
        assert_eq!(rows[2].node, 0);
        // At a both lanes converge into lane 0.
        assert_eq!(rows[3].node, 0);
        assert!(rows[3].upper.iter().any(|e| e.from == 1 && e.to == 0));
        assert_eq!(rows[3].width, 2);
    }

    #[test]
    fn freed_lanes_are_reused() {
        // Two merges, one after another: the second feature lane reuses
        // the lane the first one freed.
        let log = vec![
            commit("m2", &["m1", "g"]),
            commit("g", &["m1"]),
            commit("m1", &["a", "f"]),
            commit("f", &["a"]),
            commit("a", &[]),
        ];
        let rows = layout_graph(&log);
        assert_eq!(rows[1].node, 1);
        assert_eq!(rows[3].node, 1, "f reuses lane 1");
        assert!(rows.iter().all(|r| r.width <= 2));
    }

    #[test]
    fn parents_outside_the_list_end_their_lane() {
        let log = vec![commit("b", &["a", "zzz"]), commit("a", &["missing"])];
        let rows = layout_graph(&log);
        assert_eq!(rows[0].lower.len(), 1);
        assert!(rows[1].lower.is_empty());
    }

    #[test]
    fn independent_roots_get_separate_lanes() {
        let log = vec![commit("x", &[]), commit("y", &["z"]), commit("z", &[])];
        let rows = layout_graph(&log);
        assert_eq!(rows[0].node, 0);
        assert_eq!(rows[1].node, 0, "x ended, its lane is free again");
    }
}
