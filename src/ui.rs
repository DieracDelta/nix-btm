use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Cell, Wrap};
use ratatui::Frame;
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::{Modifier, Style},
    widgets::{Block, Paragraph, Row, Table, TableState},
};
use tui_tree_widget::Tree;

use crate::get_stats::{gen_tree, get_active_users_and_pids, ProcMetadata};
use crate::gruvbox::Gruvbox::{Dark0, OrangeBright, OrangeDim, YellowBright, YellowDim};
use crate::{App, WhichPane};

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
/// helper function to create a centered rect using up certain percentage of the available rect `r`
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

pub fn ui(f: &mut Frame, app: &mut App) {
    let border_style_selected = Style::default().fg(YellowBright.into());
    let border_style_unselected = Style::default().fg(YellowDim.into());
    let title_style_selected = Style::default()
        .fg(Dark0.into())
        .bg(YellowBright.into())
        .add_modifier(Modifier::BOLD);
    let title_style_unselected = Style::default()
        .fg(Dark0.into())
        .bg(YellowDim.into())
        .add_modifier(Modifier::BOLD);
    let user_map = get_active_users_and_pids();
    let items = gen_tree(&user_map);
    let size = f.size();
    if app.man_toggle {
        let area = centered_rect(60, 20, size);
        let text = vec![
            Line::from("q - QUIT").alignment(Alignment::Left),
            Line::from("M - TOGGLE MANUAL").alignment(Alignment::Left),
            Line::from("g - SCROLL TO TOP OF BUILDER LIST").alignment(Alignment::Left),
            Line::from("G - SCROLL TO BOTTOM OF BUILDER LIST").alignment(Alignment::Left),
            Line::from("h - MOVE TO PANEL TO THE LEFT").alignment(Alignment::Left),
            Line::from("l - MOVE TO PANEL TO THE RIGHT").alignment(Alignment::Left),
            Line::from("j - SCROLL UP BUILDER LIST").alignment(Alignment::Left),
            Line::from("k - SCROLL DOWN BUILDER LIST ").alignment(Alignment::Left),
            Line::from("< - SCROLL LEFT BUILDER INFO").alignment(Alignment::Left),
            Line::from("> - SCROLL RIGHT BUILDER LIST").alignment(Alignment::Left),
        ];
        let man = Paragraph::new(text)
            .block(
                Block::bordered()
                    .title("MANUAL")
                    .title_style(title_style_selected)
                    .border_style(border_style_selected),
            )
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        f.render_widget(man, area);
    } else {
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
                    .title("Nix builders list")
                    .title_bottom("")
                    .title_style(if app.which_pane == WhichPane::Left {
                        title_style_selected
                    } else {
                        title_style_unselected
                    })
                    .border_style(if app.which_pane == WhichPane::Left {
                        border_style_selected
                    } else {
                        border_style_unselected
                    }), // good for debugging
                        // .title_bottom(format!("{:?}", app.state)),
            )
            .highlight_style(
                Style::new()
                    .fg(Dark0.into())
                    .bg(if app.which_pane == WhichPane::Left {
                        OrangeBright.into()
                    } else {
                        OrangeDim.into()
                    })
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");
        f.render_stateful_widget(widget, chunks[0], &mut app.state);

        let mut table_state = TableState::default();
        let header = [
            "pid",
            "env",
            "parent pid",
            "p_mem",
            "v_mem",
            "runtime",
            "cmd",
        ]
        .into_iter()
        .map(Cell::from)
        .collect::<Row>();
        let mut rows = Vec::new();
        if let Some(selected) = app.state.selected().first() {
            for ProcMetadata {
                id,
                env,
                parent,
                p_mem,
                v_mem,
                run_time,
                cmd,
            } in user_map.get(selected).unwrap().iter()
            {
                rows.push(
                    [
                        &id.to_string(),
                        &env.to_vec().join(" "),
                        parent,
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
            Constraint::Percentage(if app.horizontal_scroll == 0 { 6 } else { 0 }),
            Constraint::Percentage(0),
            Constraint::Percentage(if app.horizontal_scroll <= 1 { 6 } else { 0 }),
            Constraint::Percentage(if app.horizontal_scroll <= 2 { 6 } else { 0 }),
            Constraint::Percentage(if app.horizontal_scroll <= 3 { 6 } else { 0 }),
            Constraint::Percentage(if app.horizontal_scroll <= 4 { 6 } else { 0 }),
            Constraint::Percentage(match app.horizontal_scroll {
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
                    .title_style(if app.which_pane == WhichPane::Right {
                        title_style_selected
                    } else {
                        title_style_unselected
                    })
                    .border_style(if app.which_pane == WhichPane::Right {
                        border_style_selected
                    } else {
                        border_style_unselected
                    }),
            )
            .highlight_style(Style::new());
        f.render_stateful_widget(table, chunks[1], &mut table_state);
    }
}
