//! GitKraken-style commit-graph: a pure lane-assignment layout over the commit
//! DAG, plus an egui painter that draws the colored nodes/edges + branch pills.
//!
//! The layout is deliberately separated from the rendering so the topology can
//! be unit-tested without a GPU in sight (Zen of Python: if it's hard to test,
//! the design is wrong).

use eframe::egui;

use super::Workspace;
use crate::git::Commit;

/// Branch-lane palette, cycled by color index. Picked to stay legible on a dark
/// background and distinct from each other.
const LANE_COLORS: &[egui::Color32] = &[
    egui::Color32::from_rgb(120, 170, 255), // blue
    egui::Color32::from_rgb(120, 210, 150), // green
    egui::Color32::from_rgb(230, 150, 200), // pink
    egui::Color32::from_rgb(235, 180, 100), // amber
    egui::Color32::from_rgb(170, 150, 230), // violet
    egui::Color32::from_rgb(120, 210, 215), // teal
    egui::Color32::from_rgb(225, 130, 120), // coral
    egui::Color32::from_rgb(200, 200, 130), // olive
];

/// Resolve a color index to a concrete palette color.
fn lane_color(idx: usize) -> egui::Color32 {
    LANE_COLORS[idx % LANE_COLORS.len()]
}

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

/// State for the "create branch here" modal.
pub(crate) struct BranchDialog {
    pub at: String,
    pub at_short: String,
    pub name: String,
}

/// A deferred git action requested from the graph (applied after the
/// borrow-holding scroll closure finishes).
enum GraphAction {
    Open(Commit),
    Checkout(String),
    NewBranch { at: String, short: String },
    CherryPick(String),
    Revert(String),
    Reset(String, &'static str),
    DeleteBranch(String),
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
                let target = match lanes.iter().position(|l| l.as_deref() == Some(parent.as_str()))
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

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const ROW_H: f32 = 30.0;
const LANE_W: f32 = 16.0;
const NODE_R: f32 = 5.0;
const LEFT_PAD: f32 = 12.0;

impl Workspace {
    /// Paint the commit graph (the GitKraken-style view). Clicking a row opens
    /// that commit's patch.
    pub(crate) fn render_graph(&mut self, ui: &mut egui::Ui) {
        let id = self.id.0;
        let layout = compute_graph(&self.git_graph_commits);
        if layout.rows.is_empty() {
            ui.weak("No commits to graph (is this a fresh repo?).");
            return;
        }
        let graph_w = LEFT_PAD + layout.lanes as f32 * LANE_W + 8.0;
        let mut action: Option<GraphAction> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt(("git-graph", id))
            .show(ui, |ui| {
                let row_w = ui.available_width().max(graph_w + 200.0);
                for row in &layout.rows {
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(row_w, ROW_H), egui::Sense::click());
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let painter = ui.painter_at(rect);
                    if resp.hovered() {
                        painter.rect_filled(rect, 2.0, egui::Color32::from_white_alpha(10));
                    }
                    draw_row_graph(&painter, rect, row);
                    draw_row_text(&painter, rect, row, graph_w);
                    if resp.clicked() {
                        action = Some(GraphAction::Open(row.commit.clone()));
                    }
                    row_context_menu(&resp, row, &mut action);
                    resp.on_hover_text(format!(
                        "{} · {} · {}",
                        row.commit.short, row.commit.author, row.commit.when
                    ));
                }
            });

        self.apply_graph_action(action);
        self.render_branch_dialog(ui);
    }

    /// Run a deferred graph action and refresh the view + feedback line.
    fn apply_graph_action(&mut self, action: Option<GraphAction>) {
        let Some(action) = action else { return };
        match action {
            GraphAction::Open(c) => self.open_commit(&c),
            GraphAction::Checkout(name) => {
                let r = crate::git::checkout(&self.root, &name);
                self.git_action(r, &format!("Checked out {name}"));
            }
            GraphAction::CherryPick(hash) => {
                let r = crate::git::cherry_pick(&self.root, &hash);
                self.git_action(r, "Cherry-picked commit");
            }
            GraphAction::Revert(hash) => {
                let r = crate::git::revert(&self.root, &hash);
                self.git_action(r, "Reverted commit");
            }
            GraphAction::Reset(hash, mode) => {
                let r = crate::git::reset(&self.root, &hash, mode);
                self.git_action(r, &format!("Reset {} to commit", mode.trim_start_matches('-')));
            }
            GraphAction::DeleteBranch(name) => {
                let r = crate::git::delete_branch(&self.root, &name);
                self.git_action(r, &format!("Deleted branch {name}"));
            }
            GraphAction::NewBranch { at, short } => {
                self.git_branch_dialog = Some(BranchDialog {
                    at,
                    at_short: short,
                    name: String::new(),
                });
            }
        }
    }

    /// The modal for naming a new branch created from a graph commit.
    fn render_branch_dialog(&mut self, ui: &egui::Ui) {
        let Some(mut dlg) = self.git_branch_dialog.take() else {
            return;
        };
        let mut open = true;
        let mut create = false;
        let mut cancel = false;
        egui::Window::new("Create branch")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new(format!("New branch at {}", dlg.at_short))
                        .weak()
                        .small(),
                );
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut dlg.name).hint_text("branch-name"),
                );
                resp.request_focus();
                let enter =
                    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!dlg.name.trim().is_empty(), egui::Button::new("Create"))
                        .clicked()
                        || enter
                    {
                        create = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });

        if create && !dlg.name.trim().is_empty() {
            let name = dlg.name.trim().to_string();
            let r = crate::git::create_branch(&self.root, &name, &dlg.at);
            self.git_action(r, &format!("Created branch {name}"));
        } else if !cancel && open {
            self.git_branch_dialog = Some(dlg); // keep the modal up
        }
    }
}

/// Right-click menu for a graph row: checkout/branch/cherry-pick/revert/reset.
fn row_context_menu(
    resp: &egui::Response,
    row: &GraphRow,
    action: &mut Option<GraphAction>,
) {
    let hash = row.commit.hash.clone();
    let short = row.commit.short.clone();
    resp.context_menu(|ui| {
        if ui.button("View patch").clicked() {
            *action = Some(GraphAction::Open(row.commit.clone()));
            ui.close();
        }
        // Checkout any branch/tag sitting on this commit (skip the HEAD marker).
        for r in row.commit.refs.iter().filter(|r| r.as_str() != "HEAD") {
            ui.horizontal(|ui| {
                if ui.button(format!("Checkout {r}")).clicked() {
                    *action = Some(GraphAction::Checkout(r.clone()));
                    ui.close();
                }
                // Only offer delete for local branches (no remote `/` prefix).
                if !r.contains('/')
                    && ui.small_button("Del").on_hover_text("Delete branch").clicked()
                {
                    *action = Some(GraphAction::DeleteBranch(r.clone()));
                    ui.close();
                }
            });
        }
        if ui.button("Create branch here…").clicked() {
            *action = Some(GraphAction::NewBranch {
                at: hash.clone(),
                short: short.clone(),
            });
            ui.close();
        }
        ui.separator();
        if ui.button("Cherry-pick onto current").clicked() {
            *action = Some(GraphAction::CherryPick(hash.clone()));
            ui.close();
        }
        if ui.button("Revert this commit").clicked() {
            *action = Some(GraphAction::Revert(hash.clone()));
            ui.close();
        }
        ui.menu_button("Reset current branch to here", |ui| {
            if ui.button("Soft — keep changes staged").clicked() {
                *action = Some(GraphAction::Reset(hash.clone(), "--soft"));
                ui.close();
            }
            if ui.button("Mixed — keep changes unstaged").clicked() {
                *action = Some(GraphAction::Reset(hash.clone(), "--mixed"));
                ui.close();
            }
            if ui
                .button(egui::RichText::new("Hard — DISCARD changes").color(egui::Color32::from_rgb(230, 120, 120)))
                .clicked()
            {
                *action = Some(GraphAction::Reset(hash.clone(), "--hard"));
                ui.close();
            }
        });
        ui.separator();
        if ui.button("Copy hash").clicked() {
            ui.ctx().copy_text(hash.clone());
            ui.close();
        }
    });
}

/// X-coordinate of a lane's centre line within `rect`.
fn lane_x(rect: egui::Rect, lane: usize) -> f32 {
    rect.left() + LEFT_PAD + lane as f32 * LANE_W
}

/// Draw all edges + the node for one row.
fn draw_row_graph(painter: &egui::Painter, rect: egui::Rect, row: &GraphRow) {
    let top = rect.top();
    let mid = rect.center().y;
    let bottom = rect.bottom();

    for e in &row.edges {
        let col = lane_color(e.color);
        let stroke = egui::Stroke::new(2.0, col);
        let xf = lane_x(rect, e.from);
        let xt = lane_x(rect, e.to);
        match e.half {
            EdgeHalf::Full => {
                painter.line_segment(
                    [egui::pos2(xf, top), egui::pos2(xf, bottom)],
                    stroke,
                );
            }
            EdgeHalf::Top => bezier(painter, xf, top, xt, mid, stroke),
            EdgeHalf::Bottom => bezier(painter, xf, mid, xt, bottom, stroke),
        }
    }

    let node_pos = egui::pos2(lane_x(rect, row.node_lane), mid);
    let head = row.commit.refs.iter().any(|r| r == "HEAD");
    painter.circle_filled(node_pos, NODE_R, lane_color(row.node_color));
    painter.circle_stroke(
        node_pos,
        NODE_R,
        egui::Stroke::new(
            if head { 2.0 } else { 1.0 },
            if head {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_black_alpha(160)
            },
        ),
    );
}

/// A smooth S-curve between two lane positions (degrades to a straight line
/// when `xf == xt`).
fn bezier(painter: &egui::Painter, xf: f32, y0: f32, xt: f32, y1: f32, stroke: egui::Stroke) {
    if (xf - xt).abs() < 0.5 {
        painter.line_segment([egui::pos2(xf, y0), egui::pos2(xt, y1)], stroke);
        return;
    }
    let ym = (y0 + y1) * 0.5;
    let curve = egui::epaint::CubicBezierShape::from_points_stroke(
        [
            egui::pos2(xf, y0),
            egui::pos2(xf, ym),
            egui::pos2(xt, ym),
            egui::pos2(xt, y1),
        ],
        false,
        egui::Color32::TRANSPARENT,
        stroke,
    );
    painter.add(curve);
}

/// Draw ref pills + subject (left) and short-hash/age/author (right).
fn draw_row_text(painter: &egui::Painter, rect: egui::Rect, row: &GraphRow, graph_w: f32) {
    let mid = rect.center().y;
    let font = egui::FontId::proportional(13.0);
    let mono = egui::FontId::monospace(11.0);
    let dim = egui::Color32::from_gray(150);
    let mut x = rect.left() + graph_w;

    // Ref pills (branches/tags).
    for r in &row.commit.refs {
        let is_head = r == "HEAD";
        let galley = painter.layout_no_wrap(r.clone(), font.clone(), egui::Color32::WHITE);
        let pad = egui::vec2(5.0, 1.0);
        let size = galley.size() + pad * 2.0;
        let pill = egui::Rect::from_min_size(egui::pos2(x, mid - size.y / 2.0), size);
        let bg = if is_head {
            lane_color(row.node_color)
        } else {
            egui::Color32::from_rgb(70, 78, 92)
        };
        painter.rect_filled(pill, 3.0, bg);
        painter.galley(pill.min + pad, galley, egui::Color32::WHITE);
        x = pill.right() + 5.0;
    }

    // Right-aligned metadata: short hash · age · author.
    let meta = format!("{}  {}  {}", row.commit.short, row.commit.when, row.commit.author);
    let meta_g = painter.layout_no_wrap(meta, mono, dim);
    let meta_x = rect.right() - meta_g.size().x - 8.0;
    painter.galley(
        egui::pos2(meta_x, mid - meta_g.size().y / 2.0),
        meta_g,
        dim,
    );

    // Subject, clipped so it never collides with the metadata.
    let avail = (meta_x - x - 8.0).max(20.0);
    let subj = painter.layout(
        row.commit.subject.clone(),
        font,
        egui::Color32::from_gray(225),
        avail,
    );
    painter.galley(
        egui::pos2(x, mid - subj.size().y / 2.0),
        subj,
        egui::Color32::from_gray(225),
    );
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
            merge.edges.iter().filter(|e| e.half == EdgeHalf::Bottom).count(),
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

