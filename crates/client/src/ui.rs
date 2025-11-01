use std::collections::{HashMap, HashSet, VecDeque};

use lazy_static::lazy_static;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Text},
    widgets::{Block, Cell, Paragraph, Row, Table, TableState, Tabs, Wrap},
};
use strum::IntoEnumIterator;
use tracing::{error, info};
use tui_tree_widget::{Tree, TreeItem};

use crate::{
    App, Pane, SelectedTab,
    get_stats::{ProcMetadata, gen_ui_by_nix_builder},
    gruvbox::Gruvbox::{
        self, Dark0, OrangeBright, OrangeDim, YellowBright, YellowDim,
    },
    handle_internal_json::{
        Drv, JobStatus, JobsStateInner, format_duration, format_secs,
    },
};

static NON_UNIQUE_ID_ERR_MSG: &str = "all item identifiers must be unique";

lazy_static! {
    pub static ref TITLE_STYLE_SELECTED: Style = {
        Style::default()
            .fg(Gruvbox::Dark0Hard.into())
            .bg(YellowBright.into())
            .add_modifier(Modifier::BOLD)
    };
    pub static ref TITLE_STYLE_UNSELECTED: Style = {
        Style::default()
            .fg(Gruvbox::Dark2.into())
            .bg(YellowDim.into())
            .add_modifier(Modifier::BOLD)
    };
    pub static ref TITLE_STYLE_SELECTED_SECONDARY: Style = {
        Style::default()
            .fg(Dark0.into())
            .bg(YellowBright.into())
            .add_modifier(Modifier::BOLD)
    };
    pub static ref TITLE_STYLE_UNSELECTED_SECONDARY: Style = {
        Style::default()
            .fg(Dark0.into())
            .bg(YellowDim.into())
            .add_modifier(Modifier::BOLD)
    };
    pub static ref BORDER_STYLE_SELECTED: Style =
        Style::default().fg(YellowBright.into());
    pub static ref BORDER_STYLE_UNSELECTED: Style =
        Style::default().fg(YellowDim.into());
}

const MAN_PAGE_BUILDER_VIEW: [&str; 12] = [
    "q - QUIT",
    "M - TOGGLE MANUAL",
    "g - SCROLL TO TOP OF BUILDER LIST",
    "G - SCROLL TO BOTTOM OF BUILDER LIST",
    "h - MOVE TO PANEL TO THE LEFT",
    "l - MOVE TO PANEL TO THE RIGHT",
    "j - SCROLL UP BUILDER LIST",
    "k - SCROLL DOWN BUILDER LIST ",
    "< - SCROLL LEFT BUILDER INFO",
    "> - SCROLL RIGHT BUILDER LIST",
    "p - PREVIOUS TAB",
    "n - NEXT TAB",
];

const MAN_PAGE_EAGLE_EYE_VIEW: [&str; 4] = [
    "q - QUIT",
    "M - TOGGLE MANUAL",
    "p - PREVIOUS TAB",
    "n - NEXT TAB",
];

const MAN_PAGE_BUILD_JOB_VIEW: [&str; 4] = [
    "q - QUIT",
    "M - TOGGLE MANUAL",
    "p - PREVIOUS TAB",
    "n - NEXT TAB",
];

pub fn format_bytes(size: usize) -> String {
    const MB: usize = 1024 * 1024;
    const GB: usize = 1024 * 1024 * 1024; // 1024 * 1024 * 1024

    if size >= GB {
        format!("{:.3} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.2} MB", size as f64 / MB as f64)
    } else {
        format!("{} bytes", size)
    }
}

/// borrowed from ratatui popup example
/// helper function to create a centered rect using up certain percentage of the
/// available rect `r`
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}

pub fn draw_man_page(f: &mut Frame, size: Rect, app: &mut App) {
    // TODO abstract out the map -> to_vec stuff
    let text = match app.tab_selected {
        SelectedTab::BuilderView => MAN_PAGE_BUILDER_VIEW
            .map(|s| Line::from(s).alignment(Alignment::Left))
            .to_vec(),
        SelectedTab::EagleEyeView => MAN_PAGE_EAGLE_EYE_VIEW
            .map(|s| Line::from(s).alignment(Alignment::Left))
            .to_vec(),
        SelectedTab::BuildJobView => MAN_PAGE_BUILD_JOB_VIEW
            .map(|s| Line::from(s).alignment(Alignment::Left))
            .to_vec(),
    };
    let area = centered_rect(60, 20, size);
    let man = Paragraph::new(text)
        .block(
            Block::bordered()
                .title("MANUAL")
                .title_style(*TITLE_STYLE_SELECTED)
                .border_style(*BORDER_STYLE_SELECTED)
                .fg(Gruvbox::Light1)
                .bg(Gruvbox::Dark1),
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    f.render_widget(man, area);
}

#[derive(Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum PruneType {
    None,
    Aggressive,
    #[default]
    Normal,
}

impl PruneType {
    pub fn increment(self) -> Self {
        match self {
            PruneType::None => PruneType::Normal,
            PruneType::Normal => PruneType::Aggressive,
            PruneType::Aggressive => PruneType::None,
        }
    }
}

fn reachable_active_leaves(
    d: &Drv,
    state: &JobsStateInner,
    active: &HashSet<Drv>,
    memo: &mut HashMap<Drv, HashSet<Drv>>,
) -> HashSet<Drv> {
    if let Some(s) = memo.get(d) {
        return s.clone();
    }
    if state.get_status(d).is_active() {
        let mut s = HashSet::new();
        s.insert(d.clone());
        memo.insert(d.clone(), s.clone());
        return s;
    }
    let mut out = HashSet::new();
    if let Some(node) = state.dep_tree.nodes.get(d) {
        for child in &node.deps {
            if active.contains(child) {
                let s = reachable_active_leaves(child, state, active, memo);
                out.extend(s);
            }
        }
    }
    memo.insert(d.clone(), out.clone());
    out
}

/// Aggressive mode: collapse wrappers to the first visible node.
fn collapse_to_visible_owned(
    d: &Drv,
    state: &JobsStateInner,
    active: &HashSet<Drv>,
    leaves_memo: &mut HashMap<Drv, HashSet<Drv>>,
) -> Drv {
    if state.get_status(d).is_active() {
        return d.clone();
    }
    let leaves = reachable_active_leaves(d, state, active, leaves_memo);
    match leaves.len() {
        0 => d.clone(),                          // dead end
        1 => leaves.into_iter().next().unwrap(), // jump to leaf
        _ => d.clone(),                          // branch point
    }
}

pub fn explore_root(
    root: &mut TreeItem<'_, String>,
    state: &JobsStateInner,
    root_drv: &Drv,
    prune: PruneType,
    active_closure: Option<&HashSet<Drv>>, // Some(&set) for Normal/Aggressive
) {
    // if pruning but no closure, fall back to no-prune
    let active = if prune == PruneType::None {
        None
    } else {
        active_closure
    };

    if let Some(ac) = active {
        if !ac.contains(root_drv) {
            return; // root not on a path to any active leaf
        }
    }

    let mut printed_leaves: HashSet<Drv> = HashSet::new(); // leaves rendered globally
    let mut leaves_memo: HashMap<Drv, HashSet<Drv>> = HashMap::new();

    let mut stack: Vec<(Drv, Vec<usize>)> =
        vec![(root_drv.clone(), Vec::new())];
    let mut seen_parents: HashSet<Drv> = HashSet::new();

    while let Some((parent_drv, path)) = stack.pop() {
        if !seen_parents.insert(parent_drv.clone()) {
            continue;
        }

        if let Some(children) =
            state.dep_tree.nodes.get(&parent_drv).map(|n| &n.deps)
        {
            // descend to UI node indicated by `path`
            let mut ui = &mut *root;
            for &i in &path {
                ui = ui.child_mut(i).expect("UI path out of sync");
            }

            let mut added_ids: HashSet<String> = HashSet::new(); // per-parent dedupe
            let mut idx = 0;

            // ── NORMAL mode: preselect children that add NEW active leaves at
            // this parent
            let mut kept_children: Vec<&Drv> = Vec::new();
            if let (PruneType::Normal, Some(ac)) = (prune, active) {
                let mut assigned_here: HashSet<Drv> = HashSet::new();
                for child in children {
                    if !ac.contains(child) {
                        continue;
                    }
                    if state.get_status(child).is_active() {
                        if !printed_leaves.contains(child) {
                            kept_children.push(child);
                            assigned_here.insert(child.clone());
                        }
                        continue;
                    }
                    let mut leaves = reachable_active_leaves(
                        child,
                        state,
                        ac,
                        &mut leaves_memo,
                    );
                    leaves.retain(|l| !printed_leaves.contains(l));
                    let contributes_new =
                        leaves.iter().any(|l| !assigned_here.contains(l));
                    if contributes_new {
                        assigned_here.extend(leaves);
                        kept_children.push(child);
                    }
                }
            }

            // IMPORTANT: yield &Drv, never use `.copied()`, and add an explicit
            // lifetime on the trait object.
            let iter: Box<dyn Iterator<Item = &Drv> + '_> =
                match (prune, active) {
                    (PruneType::Normal, Some(_)) => {
                        Box::new(kept_children.into_iter())
                    }
                    _ => Box::new(children.iter()), // <- yields &Drv
                };

            for child in iter {
                // Prune siblings not in closure for Aggressive (Normal already
                // filtered)
                if let (PruneType::Aggressive, Some(ac)) = (prune, active) {
                    if !ac.contains(child) {
                        continue;
                    }
                }

                // Decide rendering + traversal
                let (to_render, push_drv): (Option<Drv>, Option<Drv>) =
                    match (prune, active) {
                        (PruneType::None, _) => {
                            (Some(child.clone()), Some(child.clone()))
                        }

                        // NORMAL: show child iff it exposes ≥1 unprinted active
                        // leaf (no collapsing)
                        (PruneType::Normal, Some(ac)) => {
                            if state.get_status(child).is_active() {
                                if !printed_leaves.insert(child.clone()) {
                                    (None, None)
                                } else {
                                    (Some(child.clone()), None)
                                }
                            } else {
                                let mut leaves = reachable_active_leaves(
                                    child,
                                    state,
                                    ac,
                                    &mut leaves_memo,
                                );
                                leaves.retain(|l| !printed_leaves.contains(l));
                                if leaves.is_empty() {
                                    (None, None)
                                } else {
                                    (Some(child.clone()), Some(child.clone()))
                                }
                            }
                        }

                        // AGGRESSIVE: collapse wrappers to first visible node;
                        // still one print per leaf
                        (PruneType::Aggressive, Some(ac)) => {
                            let vis = collapse_to_visible_owned(
                                child,
                                state,
                                ac,
                                &mut leaves_memo,
                            );
                            if state.get_status(&vis).is_active() {
                                if !printed_leaves.insert(vis.clone()) {
                                    (None, None)
                                } else {
                                    (Some(vis.clone()), None)
                                }
                            } else {
                                (Some(vis.clone()), Some(vis))
                            }
                        }

                        // requested pruning but no closure → no-prune
                        (PruneType::Normal | PruneType::Aggressive, None) => {
                            (Some(child.clone()), Some(child.clone()))
                        }
                    };

                if let Some(vis) = to_render {
                    let ident = vis.to_string();
                    if !added_ids.insert(ident.clone()) {
                        continue; // per-parent duplicate
                    }

                    let node = TreeItem::new(
                        ident,
                        state.make_tree_description(&vis),
                        vec![],
                    )
                    .expect("TreeItem::new failed");

                    if ui.add_child(node).is_ok() {
                        if let Some(next) = push_drv {
                            let mut next_path = path.clone();
                            next_path.push(idx);
                            stack.push((next, next_path));
                        }
                        idx += 1;
                    } else {
                        tracing::warn!(
                            "duplicate child under {:?}, skipped",
                            parent_drv
                        );
                    }
                }
            }
        }
    }
}

// iterate through tree roots
// print drv using the tree roots
// for each drv, look up and see if there are any build jobs going on. If
// there are, then you can use that to deduce the status. If there are
// none, you take the L and say unused
pub fn gen_drv_tree_leaves_from_state(
    state: &JobsStateInner,
    do_prune: PruneType,
) -> Vec<TreeItem<'_, String>> {
    let active = if do_prune != PruneType::None {
        Some(compute_active_closure(state))
    } else {
        None
    };
    let mut roots = vec![];
    for a_root in &state.dep_tree.tree_roots {
        error!("building tree node for {a_root}");
        let mut new_root = TreeItem::new(
            a_root.clone().to_string(),
            state.make_tree_description(a_root),
            vec![],
        )
        .unwrap();
        explore_root(&mut new_root, state, a_root, do_prune, active.as_ref());
        roots.push(new_root);
    }
    info!("total number roots {} ", state.dep_tree.tree_roots.len());

    roots
}

pub fn compute_active_closure(state: &JobsStateInner) -> HashSet<Drv> {
    // Build reverse adjacency: child -> parents
    let mut rev: HashMap<&Drv, Vec<&Drv>> = HashMap::new();
    for (parent, node) in &state.dep_tree.nodes {
        for child in &node.deps {
            rev.entry(child).or_default().push(parent);
        }
    }

    // Seed with all currently active nodes (by reference to avoid cloning)
    let mut q: VecDeque<&Drv> = state
        .dep_tree
        .nodes
        .keys()
        .filter(|d| state.get_status(d).is_active())
        .collect();

    // Mark visited (by reference), then clone once at the end
    let mut marked: HashSet<&Drv> = HashSet::new();

    while let Some(d) = q.pop_front() {
        if marked.insert(d) {
            if let Some(parents) = rev.get(d) {
                for &p in parents {
                    q.push_back(p);
                }
            }
        }
    }

    // Return owned set
    marked.into_iter().cloned().collect()
}

pub fn draw_eagle_eye_ui(f: &mut Frame, size: Rect, app: &mut App) {
    let state = &app.cur_info_builds;

    error!("getting items for eagle eye ui");
    let items: Vec<TreeItem<'_, String>> =
        gen_drv_tree_leaves_from_state(state, app.eagle_eye_view.active_only);
    error!(
        "got items for eagle eye ui, there are {:?} items",
        items.len()
    );

    let chunks = Layout::horizontal([
        // just the drv tree for now
        // add a second pane later
        Constraint::Percentage(100),
    ])
    .split(size);
    // TODO don't draw if there are no derivations in progress?

    let drv_tree_widget = Tree::new(&items)
        .expect("all item id")
        .block(
            Block::bordered()
                .title(format!(
                    "Drv Building List - Pruning {:?}",
                    app.eagle_eye_view.active_only,
                ))
                //.title_bottom("")
                .title_style(app.builder_view.gen_title_style(Pane::Left))
                .border_style(app.builder_view.gen_border_style(Pane::Left))
                .bg(Gruvbox::Dark1)
                .fg(Gruvbox::Light1),
        )
        .highlight_style(
            Style::new()
                .fg(Dark0.into())
                .bg(if app.builder_view.selected_pane == Pane::Left {
                    OrangeBright.into()
                } else {
                    OrangeDim.into()
                })
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(
        drv_tree_widget,
        chunks[0],
        &mut app.eagle_eye_view.state,
    );
}

pub fn draw_builder_ui(f: &mut Frame, size: Rect, app: &mut App) {
    let user_map = &app.cur_info;

    let items = gen_ui_by_nix_builder(user_map);
    if items.is_empty() {
        return;
    }

    let chunks = Layout::horizontal([
        // title
        Constraint::Percentage(20),
        //content
        Constraint::Percentage(80),
    ])
    .split(size);

    let widget = Tree::new(&items)
        .expect("all item identifiers are unique")
        .block(
            Block::bordered()
                .title("NIX BUILDERS LIST")
                .title_bottom("")
                .title_style(app.builder_view.gen_title_style(Pane::Left))
                .border_style(app.builder_view.gen_border_style(Pane::Left))
                .bg(Gruvbox::Dark1)
                .fg(Gruvbox::Light1),
        )
        .highlight_style(
            Style::new()
                .fg(Dark0.into())
                .bg(if app.builder_view.selected_pane == Pane::Left {
                    OrangeBright.into()
                } else {
                    OrangeDim.into()
                })
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    f.render_stateful_widget(widget, chunks[0], &mut app.builder_view.state);

    let mut table_state = TableState::default();
    let header = ["pid", "env", "parent pid", "p_mem", "v_mem", "⏰", "cmd"]
        .into_iter()
        .map(Cell::from)
        .collect::<Row>();
    let mut rows = Vec::new();
    if let Some(selected) = app.builder_view.state.selected().first() {
        for ProcMetadata {
            id,
            env,
            parent,
            p_mem,
            v_mem,
            run_time,
            cmd,
            owner: _name,
        } in user_map.get(selected).unwrap().iter()
        {
            rows.push(
                [
                    &id.to_string(),
                    &env.to_vec().join(" "),
                    &(*parent).unwrap().to_string(),
                    &format_bytes(*p_mem as usize),
                    &format_bytes(*v_mem as usize),
                    &format_secs(*run_time),
                    &cmd.iter().take(8).cloned().collect::<Vec<_>>().join(
                        "
     ",
                    ),
                ]
                .into_iter()
                .map(|content| Cell::from(Text::from(content.to_string())))
                .collect::<Row>(),
            )
        }
    }

    let widths = [
        Constraint::Percentage(if app.builder_view.horizontal_scroll == 0 {
            6
        } else {
            0
        }),
        Constraint::Percentage(0),
        Constraint::Percentage(if app.builder_view.horizontal_scroll <= 1 {
            6
        } else {
            0
        }),
        Constraint::Percentage(if app.builder_view.horizontal_scroll <= 2 {
            6
        } else {
            0
        }),
        Constraint::Percentage(if app.builder_view.horizontal_scroll <= 3 {
            6
        } else {
            0
        }),
        Constraint::Percentage(if app.builder_view.horizontal_scroll <= 4 {
            6
        } else {
            0
        }),
        Constraint::Percentage(match app.builder_view.horizontal_scroll {
            0 => 69,
            1 => 75,
            2 => 81,
            3 => 87,
            4 => 93,
            _ => 100,
        }),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::bordered()
                .title("BUILDER INFO")
                .title_bottom("M TO TOGGLE MANUAL")
                .title_style(app.builder_view.gen_title_style(Pane::Right))
                .border_style(app.builder_view.gen_border_style(Pane::Right))
                .bg(Gruvbox::Dark1)
                .fg(Gruvbox::Light3),
        )
        .row_highlight_style(Style::new().fg(Gruvbox::Light3.into()));
    f.render_stateful_widget(table, chunks[1], &mut table_state);
}

pub fn render_title(f: &mut Frame, area: Rect, s: &str) {
    f.render_widget(
        Paragraph::new(s)
            .bold()
            .centered()
            .block(Block::new().bg(Gruvbox::Dark0).fg(Gruvbox::Light1)),
        area,
    );
}

pub fn render_tab(f: &mut Frame, area: Rect, app: &mut App) {
    // (text color, background color)
    let highlight_style: (Color, Color) =
        (Gruvbox::Light3.into(), Gruvbox::Dark0.into());
    let tab_style: (Color, Color) =
        (Gruvbox::Light3.into(), Gruvbox::Dark1.into());
    let titles = SelectedTab::iter()
        .map(SelectedTab::title)
        .map(|x| x.style(Style::new().bg(Gruvbox::Dark3.into())));

    let selected_tab_index = app.tab_selected as usize;
    f.render_widget(
        Tabs::new(titles)
            .style(tab_style)
            .highlight_style(highlight_style)
            .select(selected_tab_index)
            .padding("", "")
            .divider(""),
        area,
    );
}

pub fn ui(f: &mut Frame, app: &mut App) {
    use Constraint::*;
    let size = f.area();
    let vertical = Layout::vertical([Length(2), Min(0)]);
    let [header_area, inner_area] = vertical.areas(size);
    let horizontal = Layout::horizontal([Min(0), Length(20)]);
    let [tabs_area, title_area] = horizontal.areas(header_area);

    match app.tab_selected {
        SelectedTab::BuilderView => {
            render_title(f, title_area, "Builder View");
            render_tab(f, tabs_area, app);
            if app.builder_view.man_toggle {
                draw_man_page(f, inner_area, app);
            } else {
                draw_builder_ui(f, inner_area, app)
            }
        }
        SelectedTab::EagleEyeView => {
            render_tab(f, tabs_area, app);
            render_title(f, title_area, "Eagle Eye View 🦅");
            if app.eagle_eye_view.man_toggle {
                draw_man_page(f, inner_area, app);
            } else {
                draw_eagle_eye_ui(f, inner_area, app);
            }
        }
        SelectedTab::BuildJobView => {
            render_tab(f, tabs_area, app);
            render_title(f, title_area, "Build Job View");
            if app.build_job_view.man_toggle {
                draw_man_page(f, inner_area, app);
            } else {
                draw_build_job_view_ui(f, inner_area, app);
            }
        }
    }
}

fn draw_build_job_view_ui(f: &mut Frame, inner_area: Rect, app: &mut App) {
    let chunks = Layout::horizontal([
        // everything
        Constraint::Percentage(100),
    ])
    .split(inner_area);
    let mut table_state = TableState::default();
    let header = Row::new(
        ["job id", "drv name", "status", "⏰", "drv hash"]
            .into_iter()
            .map(Cell::from),
    );
    //.style(*TITLE_STYLE_UNSELECTED);

    let widths = [
        Constraint::Percentage(20),
        Constraint::Percentage(30),
        Constraint::Percentage(20),
        Constraint::Percentage(10),
        Constraint::Percentage(20),
    ];
    let rows: Vec<_> = app
        .cur_info_builds
        .jid_to_job
        .clone()
        .into_iter()
        .map(|(id, job)| {
            [
                id.to_string(),
                job.drv.name.clone(),
                job.status.to_string(),
                format_duration(job.runtime()),
                job.drv.hash,
            ]
            .into_iter()
            .map(|content| Cell::from(Text::from(content)))
            .collect::<Row>()
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::bordered()
                .title("JOB INFO")
                .title_bottom("M TO TOGGLE MANUAL")
                .title_style(app.builder_view.gen_title_style(Pane::Right))
                .border_style(app.builder_view.gen_border_style(Pane::Right))
                .bg(Gruvbox::Dark1)
                .fg(Gruvbox::Light3),
        )
        .row_highlight_style(Style::new().fg(Gruvbox::Light3.into()));
    f.render_stateful_widget(table, chunks[0], &mut table_state);
}
