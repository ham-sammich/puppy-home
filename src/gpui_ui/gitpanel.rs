//! The Git (Source Control) surface: branch header + network ops, staging
//! lists, commit box, history (flat list or the lane-painted commit graph),
//! single-commit patch view, blame view, and the HTTPS credentials modal.
//! Ports `workspace/git_view.rs` + `git_graph_view.rs` + `git_creds.rs`
//! over the shared frontend-agnostic Workspace methods.

use gpui::{
    AnyElement, Bounds, Entity, FontWeight, IntoElement, ParentElement as _, Path as GpuiPath,
    Rgba, Styled as _, div, fill, point, prelude::*, px, size,
};

use crate::gpui_ui::chat::transcript::diff_body;
use crate::gpui_ui::input::ChatInput;
use crate::gpui_ui::widgets::{self, alpha};
use crate::gpui_ui::{DashAction, RootView, Tokens};
use crate::workspace::{EdgeHalf, GraphRow, Workspace, compute_graph};

/// Commit-box height: OUR constant, never content-derived (the 31a6dcb
/// lesson — a content-fed height crept under fractional DPI on egui; a
/// fixed constant can't creep anywhere).
const COMMIT_BOX_H: f32 = 96.0;
/// Graph geometry (egui painter parity).
const ROW_H: f32 = 30.0;
const LANE_W: f32 = 16.0;
const NODE_R: f32 = 5.0;
const LEFT_PAD: f32 = 12.0;

/// Lane palette (mirrors the egui LANE_COLORS rotation in spirit).
const LANE_COLORS: [u32; 8] = [
    0xe7ab4d, 0x6aa8ff, 0x5fd190, 0xdd9ce6, 0xf28585, 0x56c7c2, 0xb79cff, 0xe58fb0,
];

pub fn lane_rgba(idx: usize) -> Rgba {
    gpui::rgb(LANE_COLORS[idx % LANE_COLORS.len()])
}

pub struct GitArgs<'a> {
    pub t: Tokens,
    pub ws: &'a Workspace,
    pub root: Entity<RootView>,
    pub commit_input: Option<&'a Entity<ChatInput>>,
    /// List mode when true (graph is the default, like egui).
    pub list_mode: bool,
    /// Right-clicked graph row context target (commit hash, short, refs).
    pub graph_menu: Option<&'a (String, String, Vec<String>)>,
    /// Branch-name input shown while "new branch" is armed.
    pub branch_input: Option<&'a Entity<ChatInput>>,
    pub branch_armed: bool,
}

/// The Git tab body.
pub fn git_view(args: &GitArgs) -> AnyElement {
    let t = args.t;
    let ws = args.ws;
    let Some(view) = ws.git_view_data() else {
        return div()
            .p_3()
            .text_size(px(12.))
            .text_color(t.weak)
            .child("Loading git status\u{2026}")
            .into_any_element();
    };
    let id = ws.id;

    // Branch header + network ops + List/Graph toggle.
    let mut header = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(12.5))
                .text_color(t.text)
                .child(format!("\u{2387} {}", view.branch)),
        );
    if view.upstream {
        if view.ahead > 0 {
            header = header.child(small(&t, format!("\u{2191}{}", view.ahead), t.run));
        }
        if view.behind > 0 {
            header = header.child(small(&t, format!("\u{2193}{}", view.behind), t.paused));
        }
        if view.ahead == 0 && view.behind == 0 {
            header = header.child(small(&t, "up to date".into(), t.dim));
        }
    } else {
        header = header.child(small(&t, "no upstream".into(), t.dim));
    }
    header = header
        .child(div().flex_1())
        .child(act_btn(
            args,
            "\u{27f3}",
            "git-refresh",
            DashAction::GitRefresh(id),
        ))
        .child(act_btn(
            args,
            "Fetch",
            "git-fetch",
            DashAction::GitFetch(id),
        ))
        .child(act_btn(args, "Pull", "git-pull", DashAction::GitPull(id)))
        .child(act_btn(args, "Push", "git-push", DashAction::GitPush(id)))
        .child(act_btn(
            args,
            if args.list_mode { "Graph" } else { "List" },
            "git-toggle-graph",
            DashAction::GitToggleGraph(id),
        ));

    let action_msg = ws.git_action_message().map(|(ok, msg)| {
        div()
            .px_2()
            .text_size(px(11.))
            .text_color(if *ok { t.run } else { t.error })
            .child(msg.clone())
    });

    // Commit box (fixed height) + button row.
    let staged_n = view.staged.len();
    let commit_box = div()
        .flex_none()
        .h(px(COMMIT_BOX_H))
        .mx_2()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex_1()
                .min_h_0()
                .px_2()
                .py_1()
                .rounded(px(8.))
                .bg(t.well)
                .border_1()
                .border_color(t.line_soft)
                .id(("commit-box-scroll", id.0))
                .overflow_y_scroll()
                .font_family("JetBrains Mono")
                .text_size(px(12.))
                .children(args.commit_input.cloned()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    widgets::primary_btn(&t, "\u{2713} Commit")
                        .id(("git-commit", id.0))
                        .on_click({
                            let root = args.root.clone();
                            move |_, _, cx| {
                                root.update(cx, |r, cx| r.dispatch(DashAction::GitCommit(id), cx));
                            }
                        }),
                )
                .child(small(&t, format!("{staged_n} staged"), t.weak)),
        );

    // Staged + unstaged lists.
    let staged_rows: Vec<(String, char)> = view
        .staged
        .iter()
        .map(|e| (e.path.clone(), e.marker()))
        .collect();
    let unstaged_rows: Vec<(String, char)> = view
        .unstaged
        .iter()
        .map(|e| (e.path.clone(), e.marker()))
        .collect();
    // Left column: staged + unstaged lists, filling the column height with
    // their own scroll (G3 layout: two-up files | history instead of stacked).
    let staging = div()
        .flex_1()
        .min_h_0()
        .id(("git-stage-scroll", id.0))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .child(stage_section(args, "Staged", staged_rows, true))
        .child(stage_section(args, "Changes", unstaged_rows, false));

    // History (list or graph).
    let history: AnyElement = if args.list_mode {
        history_list(args, &view.log)
    } else {
        graph_view(args)
    };

    // Two-column body: files (left) | history-graph (right). The top section
    // (header, commit box, branch/menu) stays full-width above both columns.
    let columns = div()
        .flex_1()
        .min_h_0()
        .flex()
        .gap_2()
        .px_2()
        .child(
            div()
                .w(px(320.))
                .flex_none()
                .min_h_0()
                .flex()
                .flex_col()
                .border_r_1()
                .border_color(t.line_soft)
                .pr_2()
                .child(staging),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(div().pb_1().child(small(&t, "HISTORY".into(), t.weak)))
                .child(history),
        );

    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(header)
        .children(action_msg)
        .child(commit_box)
        .children(args.branch_armed.then(|| branch_input_row(args)))
        .children(args.graph_menu.map(|m| graph_menu_panel(args, m)))
        .child(columns)
        .into_any_element()
}

fn small(_t: &Tokens, text: String, color: Rgba) -> gpui::Div {
    div().text_size(px(10.5)).text_color(color).child(text)
}

fn act_btn(args: &GitArgs, label: &str, key: &'static str, action: DashAction) -> AnyElement {
    let root = args.root.clone();
    widgets::btn(&args.t, label)
        .id((key, args.ws.id.0))
        .on_click(move |_, _, cx| {
            let a = action.clone();
            root.update(cx, |r, cx| r.dispatch(a, cx));
        })
        .into_any_element()
}

/// One staging section: header (+ stage/unstage-all) and per-file rows
/// (marker, path, the +/− action; row click = diff preview).
fn stage_section(
    args: &GitArgs,
    title: &str,
    rows: Vec<(String, char)>,
    staged: bool,
) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    let all_action = if staged {
        DashAction::GitUnstageAll(id)
    } else {
        DashAction::GitStageAll(id)
    };
    let all_label = if staged { "Unstage all" } else { "Stage all" };
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .py_0p5()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(11.))
                        .text_color(t.text)
                        .child(format!("{title} ({})", rows.len())),
                )
                .children(
                    (!rows.is_empty()).then(|| act_btn(args, all_label, "git-all", all_action)),
                ),
        )
        .children(rows.is_empty().then(|| {
            small(
                &t,
                if staged {
                    "Nothing staged.".into()
                } else {
                    "No unstaged changes.".into()
                },
                t.dim,
            )
        }))
        .children(rows.into_iter().enumerate().map(|(i, (path, marker))| {
            let root_btn = args.root.clone();
            let root_row = args.root.clone();
            let p1 = path.clone();
            let p2 = path.clone();
            let toggle = if staged {
                DashAction::GitUnstage(id, p1)
            } else {
                DashAction::GitStage(id, p1)
            };
            // NOTE: p1 moved above; rebuild for closure clarity.
            let _ = &p2;
            div()
                .id((if staged { "staged-row" } else { "unstaged-row" }, i as u64))
                .flex()
                .items_center()
                .gap_1p5()
                .px_1()
                .py_0p5()
                .font_family("JetBrains Mono")
                .text_size(px(11.))
                .cursor_pointer()
                .hover(|d| d.bg(t.well))
                .child(
                    div()
                        .id((if staged { "unstage-btn" } else { "stage-btn" }, i as u64))
                        .w(px(16.))
                        .flex_none()
                        .text_color(t.accent)
                        .cursor_pointer()
                        .hover(|d| d.text_color(t.text))
                        .child(if staged { "\u{2212}" } else { "\u{ff0b}" })
                        .on_click(move |_, _, cx| {
                            let a = toggle.clone();
                            root_btn.update(cx, |r, cx| r.dispatch(a, cx));
                        }),
                )
                .child(
                    div()
                        .w(px(12.))
                        .flex_none()
                        .text_color(crate::gpui_ui::editor::marker_color(&t, marker))
                        .child(marker.to_string()),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(t.text)
                        .child(path.clone()),
                )
                .on_click(move |_, _, cx| {
                    root_row.update(cx, |r, cx| {
                        r.dispatch(DashAction::LoadGitChange(id, p2.clone(), marker), cx)
                    });
                })
                .into_any_element()
        }))
        .into_any_element()
}

/// Flat history list (50 commits): short hash, subject, refs, author, when.
fn history_list(args: &GitArgs, log: &[crate::git::Commit]) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    div()
        .id(("git-log-scroll", id.0))
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .px_2()
        .children(log.iter().enumerate().map(|(i, c)| {
            let root = args.root.clone();
            commit_row_text(&t, c)
                .id(("git-log-row", i as u64))
                .cursor_pointer()
                .hover(|d| d.bg(t.well))
                .on_click(move |_, _, cx| {
                    root.update(cx, |r, cx| {
                        r.dispatch(DashAction::GitOpenCommit(id, i, true), cx)
                    });
                })
                .into_any_element()
        }))
        .into_any_element()
}

fn commit_row_text(t: &Tokens, c: &crate::git::Commit) -> gpui::Stateful<gpui::Div> {
    div()
        .id("placeholder")
        .flex()
        .items_center()
        .gap_2()
        .py_0p5()
        .child(
            div()
                .font_family("JetBrains Mono")
                .text_size(px(10.5))
                .text_color(t.dim)
                .child(c.short.clone()),
        )
        .children(c.refs.iter().take(3).map(|r| {
            div()
                .px_1()
                .rounded(px(4.))
                .bg(alpha(t.accent, 0.15))
                .font_family("JetBrains Mono")
                .text_size(px(9.5))
                .text_color(t.accent)
                .child(r.clone())
        }))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .text_size(px(11.5))
                .text_color(t.text)
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(c.subject.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(10.))
                .text_color(t.weak)
                .child(format!("{} \u{b7} {}", c.author, c.when)),
        )
}

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

/// The lane-painted commit graph: one row per commit, a small canvas on the
/// left painting that row's edges/node (bezier halves, egui parity), text
/// to the right. Click = open commit; right-click = action panel.
fn graph_view(args: &GitArgs) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    let layout = compute_graph(args.ws.graph_commits());
    let graph_w = LEFT_PAD + layout.lanes.max(1) as f32 * LANE_W + 8.0;
    div()
        .id(("git-graph-scroll", id.0))
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .px_2()
        .children(layout.rows.into_iter().enumerate().map(|(i, row)| {
            let root_open = args.root.clone();
            let root_menu = args.root.clone();
            let refs = row.commit.refs.clone();
            let (hash, short) = (row.commit.hash.clone(), row.commit.short.clone());
            let row_el = div()
                .id(("graph-row", i as u64))
                .h(px(ROW_H))
                .flex()
                .items_center()
                .cursor_pointer()
                .hover(|d| d.bg(t.well))
                .child(row_canvas(graph_w, row.clone()))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .child(commit_row_text(&t, &row.commit)),
                )
                .on_click(move |_, _, cx| {
                    root_open.update(cx, |r, cx| {
                        r.dispatch(DashAction::GitOpenCommit(id, i, false), cx)
                    });
                })
                .on_mouse_down(gpui::MouseButton::Right, move |_, _, cx| {
                    let target = (hash.clone(), short.clone(), refs.clone());
                    root_menu.update(cx, |r, cx| {
                        r.dispatch(DashAction::GraphMenu(id, target), cx)
                    });
                });
            row_el.into_any_element()
        }))
        .into_any_element()
}

/// One row's graph cell: pass-through lines, converging/diverging bezier
/// halves, and the commit node (a circle = fully-rounded quad).
fn row_canvas(width: f32, row: GraphRow) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds: Bounds<gpui::Pixels>, _, window, _| {
            let ox = f32::from(bounds.origin.x);
            let oy = f32::from(bounds.origin.y);
            let h = ROW_H;
            let lane_x = |lane: usize| ox + LEFT_PAD + lane as f32 * LANE_W + LANE_W / 2.0;
            let node_x = lane_x(row.node_lane);
            let node_y = oy + h / 2.0;
            for e in &row.edges {
                let color = lane_rgba(e.color);
                let (x0, y0, x1, y1) = match e.half {
                    EdgeHalf::Full => (lane_x(e.from), oy, lane_x(e.to), oy + h),
                    EdgeHalf::Top => (lane_x(e.from), oy, node_x, node_y),
                    EdgeHalf::Bottom => (node_x, node_y, lane_x(e.to), oy + h),
                };
                paint_edge(window, x0, y0, x1, y1, color);
            }
            // The node: a circle via a fully-rounded quad.
            let node = fill(
                Bounds::new(
                    point(px(node_x - NODE_R), px(node_y - NODE_R)),
                    size(px(NODE_R * 2.0), px(NODE_R * 2.0)),
                ),
                lane_rgba(row.node_color),
            )
            .corner_radii(gpui::Corners::all(px(NODE_R)));
            window.paint_quad(node);
        },
    )
    .w(px(width))
    .h(px(ROW_H))
    .flex_none()
}

/// A 1.6px edge: straight when vertical, else a sampled cubic bezier band
/// (control points pull vertically — the egui painter's curve shape).
fn paint_edge(window: &mut gpui::Window, x0: f32, y0: f32, x1: f32, y1: f32, color: Rgba) {
    const SEGS: usize = 10;
    let mut pts = Vec::with_capacity(SEGS + 1);
    if (x0 - x1).abs() < 0.5 {
        pts.push((x0, y0));
        pts.push((x1, y1));
    } else {
        let my = (y0 + y1) / 2.0;
        for s in 0..=SEGS {
            let u = s as f32 / SEGS as f32;
            let v = 1.0 - u;
            // Cubic with CP1=(x0, my), CP2=(x1, my).
            let x = v * v * v * x0 + 3.0 * v * v * u * x0 + 3.0 * v * u * u * x1 + u * u * u * x1;
            let y = v * v * v * y0 + 3.0 * v * v * u * my + 3.0 * v * u * u * my + u * u * u * y1;
            pts.push((x, y));
        }
    }
    let mut path = GpuiPath::new(point(px(pts[0].0), px(pts[0].1)));
    for &(x, y) in pts.iter().skip(1) {
        path.line_to(point(px(x), px(y)));
    }
    for &(x, y) in pts.iter().rev() {
        path.line_to(point(px(x + 1.6), px(y)));
    }
    window.paint_path(path, color);
}

/// Inline action panel for a right-clicked graph row (egui's context menu).
fn graph_menu_panel(args: &GitArgs, target: &(String, String, Vec<String>)) -> AnyElement {
    let t = args.t;
    let id = args.ws.id;
    let (hash, short, refs) = target;
    let mk = |label: String, key: &'static str, idx: u64, action: DashAction| {
        let root = args.root.clone();
        div()
            .id((key, idx))
            .px_1p5()
            .py_0p5()
            .rounded(px(6.))
            .text_size(px(10.5))
            .text_color(t.text)
            .cursor_pointer()
            .hover(|d| d.bg(t.well))
            .child(label)
            .on_click(move |_, _, cx| {
                let a = action.clone();
                root.update(cx, |r, cx| r.dispatch(a, cx));
            })
            .into_any_element()
    };
    let mut items: Vec<AnyElement> = vec![mk(
        format!("checkout {short}"),
        "g-co",
        0,
        DashAction::GraphCheckout(id, hash.clone()),
    )];
    for (ri, r) in refs.iter().take(2).enumerate() {
        items.push(mk(
            format!("checkout {r}"),
            "g-co-ref",
            ri as u64,
            DashAction::GraphCheckout(id, r.clone()),
        ));
        items.push(mk(
            format!("merge {r}"),
            "g-merge",
            ri as u64,
            DashAction::GraphMerge(id, r.clone()),
        ));
    }
    items.push(mk(
        "new branch here".into(),
        "g-branch",
        0,
        DashAction::GraphNewBranch(id, hash.clone()),
    ));
    items.push(mk(
        "cherry-pick".into(),
        "g-cherry",
        0,
        DashAction::GraphCherryPick(id, hash.clone()),
    ));
    items.push(mk(
        "revert".into(),
        "g-revert",
        0,
        DashAction::GraphRevert(id, hash.clone()),
    ));
    items.push(mk(
        "reset --hard".into(),
        "g-reset",
        0,
        DashAction::GraphReset(id, hash.clone()),
    ));
    items.push(mk(
        "\u{2715}".into(),
        "g-close",
        0,
        DashAction::GraphMenuClose,
    ));
    div()
        .mx_2()
        .p_1p5()
        .rounded(px(8.))
        .bg(t.well)
        .border_1()
        .border_color(alpha(t.accent, 0.5))
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .child(
            div()
                .font_family("JetBrains Mono")
                .text_size(px(10.))
                .text_color(t.weak)
                .child(short.clone()),
        )
        .children(items)
        .into_any_element()
}

fn branch_input_row(args: &GitArgs) -> AnyElement {
    let t = args.t;
    let Some(input) = args.branch_input else {
        return div().into_any_element();
    };
    div()
        .mx_2()
        .px_2()
        .py_1()
        .rounded(px(8.))
        .bg(t.card)
        .border_1()
        .border_color(alpha(t.accent, 0.5))
        .font_family("JetBrains Mono")
        .text_size(px(11.5))
        .child(input.clone())
        .into_any_element()
}

/// A single commit's patch (the Commit editor tab).
pub fn commit_view(args: &GitArgs, hash: &str) -> AnyElement {
    let t = args.t;
    match args.ws.commit_view_data() {
        Some((h, rec)) if h == hash => div()
            .id(("commit-scroll", args.ws.id.0))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1p5()
            .p_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_family("JetBrains Mono")
                            .text_size(px(11.5))
                            .text_color(t.text)
                            .child(rec.path.clone()),
                    )
                    .child(div().flex_1())
                    .child(small(
                        &t,
                        format!("+{} \u{2212}{}", rec.adds, rec.dels),
                        t.weak,
                    )),
            )
            .child(diff_body(&t, &rec.lines))
            .into_any_element(),
        _ => div()
            .p_3()
            .text_size(px(12.))
            .text_color(t.dim)
            .child("Commit not loaded.")
            .into_any_element(),
    }
}

/// Read-only blame view: hash/author/date gutter + the code line (egui's
/// render_blame_view shape). Capped at 2000 rows.
pub fn blame_view(t: &Tokens, ws: &Workspace, path: &std::path::Path) -> AnyElement {
    let Some(lines) = ws.blame_lines(path) else {
        return div()
            .p_3()
            .text_size(px(12.))
            .text_color(t.dim)
            .child("No blame data.")
            .into_any_element();
    };
    div()
        .id(("blame-scroll", ws.id.0))
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .overflow_x_scroll()
        .flex()
        .flex_col()
        .px_2()
        .font_family("JetBrains Mono")
        .text_size(px(11.))
        .children(lines.iter().take(2000).map(|b| {
            div()
                .flex()
                .gap_2()
                .whitespace_nowrap()
                .child(
                    div()
                        .w(px(220.))
                        .flex_none()
                        .text_color(t.dim)
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(format!("{} {} {}", b.short, b.author, b.date)),
                )
                .child(div().text_color(t.text).child(b.line.clone()))
        }))
        .into_any_element()
}

/// The HTTPS credentials modal (push/pull/fetch hit an auth wall).
pub struct CredsArgs<'a> {
    pub t: Tokens,
    pub ws: &'a Workspace,
    pub root: Entity<RootView>,
    pub user_input: Option<&'a Entity<ChatInput>>,
    pub pass_input: Option<&'a Entity<ChatInput>>,
}

pub fn creds_overlay(args: &CredsArgs) -> AnyElement {
    let t = args.t;
    let Some(prompt) = args.ws.git_creds_prompt() else {
        return div().into_any_element();
    };
    let id = args.ws.id;
    let field = |label: &str, input: Option<&Entity<ChatInput>>| {
        div()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(small(&t, label.to_string(), t.weak))
            .children(input.map(|i| {
                div()
                    .px_2()
                    .py_1()
                    .rounded(px(8.))
                    .bg(t.well)
                    .border_1()
                    .border_color(t.line_soft)
                    .font_family("JetBrains Mono")
                    .text_size(px(11.5))
                    .child(i.clone())
            }))
    };
    let panel = div()
        .occlude()
        .w(px(380.))
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(13.))
        .bg(t.panel)
        .border_1()
        .border_color(alpha(t.wait, 0.55))
        .shadow_lg()
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_size(px(13.))
                .text_color(t.text)
                .child(format!(
                    "HTTPS credentials \u{2014} git {}",
                    prompt.op.verb()
                )),
        )
        .child(small(
            &t,
            "Token-as-password works (nothing is stored). Note: the password \
             field is visible while typed."
                .into(),
            t.weak,
        ))
        .children(prompt.error.clone().map(|e| small(&t, e, t.error)))
        .child(field("username", args.user_input))
        .child(field("password / token", args.pass_input))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    widgets::primary_btn(&t, "Retry")
                        .id(("creds-retry", id.0))
                        .on_click({
                            let root = args.root.clone();
                            move |_, _, cx| {
                                root.update(cx, |r, cx| {
                                    r.dispatch(DashAction::CredsSubmit(id), cx)
                                });
                            }
                        }),
                )
                .child(
                    widgets::btn(&t, "Cancel")
                        .id(("creds-cancel", id.0))
                        .on_click({
                            let root = args.root.clone();
                            move |_, _, cx| {
                                root.update(cx, |r, cx| {
                                    r.dispatch(DashAction::CredsCancel(id), cx)
                                });
                            }
                        }),
                ),
        );
    gpui::deferred(
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(alpha(t.bg, 0.6))
            .child(panel),
    )
    .with_priority(220)
    .into_any_element()
}
