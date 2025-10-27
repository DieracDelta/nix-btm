use std::collections::HashSet;

use lazy_static::lazy_static;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Text},
    widgets::{Block, Cell, Paragraph, Row, Table, TableState, Tabs, Wrap},
};
use strum::IntoEnumIterator;
use tracing::error;
use tui_tree_widget::{Tree, TreeItem};

use crate::{
    App, Pane, SelectedTab,
    get_stats::{ProcMetadata, gen_ui_by_nix_builder},
    gruvbox::Gruvbox::{
        self, Dark0, OrangeBright, OrangeDim, YellowBright, YellowDim,
    },
    handle_internal_json::{Drv, JobsStateInner, format_duration, format_secs},
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

pub fn explore_root(
    root: &mut TreeItem<'_, String>,
    state: &JobsStateInner,
    root_drv: &Drv,
) {
    let mut new_nodes = vec![(root_drv, vec![])];
    let mut seen_drvs = HashSet::<&Drv>::new();
    while let Some((parent_drv, vec_path)) = new_nodes.pop() {
        seen_drvs.insert(parent_drv);
        if let Some(children) =
            state.dep_tree.nodes.get(parent_drv).map(|x| &x.deps)
        {
            error!("working through node {parent_drv:?}");
            // this is so gross. TODO is there a better way to do this?
            let mut parent_tree_node = &mut *root;
            for idx in &vec_path {
                parent_tree_node = parent_tree_node.child_mut(*idx).unwrap();
            }
            let mut idx = 0;
            for a_child in children.iter() {
                if seen_drvs.contains(&a_child) {
                    error!("already saw {a_child}, skipping");
                    continue;
                }

                error!("adding {a_child} depency of {parent_drv} ");
                let identifier = a_child.clone().to_string();
                // TODO there needs to be progress in here
                let new_node = TreeItem::new(
                    identifier,
                    a_child.name.clone().to_string(),
                    vec![],
                )
                .unwrap();

                parent_tree_node.add_child(new_node).unwrap();
                let mut new_path = vec_path.clone();
                new_path.push(idx);
                new_nodes.push((a_child, new_path));
                idx += 1;
            }
        }
    }
}

// iterate through tree roots
// print drv using the tree roots
// ---- unimplemented part:
// for each drv, look up and see if there are any build jobs going on. If
// there are, then you can use that to deduce the status. If there are
// none, you take the L and say unused
pub fn gen_drv_tree_leaves_from_state(
    state: &JobsStateInner,
) -> Vec<TreeItem<'_, String>> {
    let mut roots = vec![];
    for a_root in &state.dep_tree.tree_roots {
        error!("building tree node for {a_root}");
        let mut new_root = TreeItem::new(
            a_root.clone().to_string(),
            a_root.name.clone().to_string(),
            vec![],
        )
        .unwrap();
        explore_root(&mut new_root, state, a_root);
        roots.push(new_root);
    }

    roots
}

pub fn draw_eagle_eye_ui(f: &mut Frame, size: Rect, app: &mut App) {
    let state = &app.cur_info_builds;

    error!("getting items for eagle eye ui");
    let items: Vec<TreeItem<'_, String>> =
        gen_drv_tree_leaves_from_state(state);
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
                .title("Drv Building List")
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
            render_title(f, title_area, "Eagle Eye View");
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
