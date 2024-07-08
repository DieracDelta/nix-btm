use lazy_static::lazy_static;
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Styled, Stylize},
    text::{Line, Text},
    widgets::{Block, Cell, Paragraph, Row, Table, TableState, Tabs, Wrap},
    Frame,
};
use strum::IntoEnumIterator;
use tui_tree_widget::Tree;

use crate::{
    get_stats::{
        gen_ui_by_nix_builder, get_active_users_and_pids, ProcMetadata,
    },
    gruvbox::Gruvbox::{
        self, Dark0, OrangeBright, OrangeDim, YellowBright, YellowDim,
    },
    App, Pane, SelectedTab,
};

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

const MAN_PAGE_BIRDS_EYE_VIEW: [&str; 4] = [
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
        SelectedTab::BirdsEyeView => MAN_PAGE_BIRDS_EYE_VIEW
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

pub fn draw_builder_ui(f: &mut Frame, size: Rect, app: &mut App) {
    let user_map = get_active_users_and_pids();
    let items = gen_ui_by_nix_builder(&user_map);
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
                    &format!("{}s", run_time),
                    &cmd.iter().take(8).cloned().collect::<Vec<_>>().join(" "),
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
        .highlight_style(Style::new().fg(Gruvbox::Light3.into()));
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
    let size = f.size();
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
        SelectedTab::BirdsEyeView => {
            render_tab(f, tabs_area, app);
            render_title(f, title_area, "Birds Eye View");
            if app.birds_eye_view.man_toggle {
                draw_man_page(f, inner_area, app);
            } else {
                draw_birds_eye_ui(f, inner_area, app);
            }
        }
    }
}

fn draw_birds_eye_ui(f: &mut Frame, inner_area: Rect, app: &mut App) {
    // todo!()
}
