use ratatui::text::{Line, Masked, Span, Text};
use ratatui::widgets::Cell;
use ratatui::Frame;
use ratatui::{
    layout::{Alignment, Constraint, Layout, Margin},
    style::{Color, Modifier, Style, Stylize},
    symbols::scrollbar,
    widgets::{Block, Paragraph, Row, Scrollbar, ScrollbarOrientation, Table, TableState},
};
use tui_tree_widget::Tree;

use crate::{App, WhichPane};
use crate::{
    get_stats::{gen_tree, get_active_users_and_pids, ProcMetadata},
};
use crate::gruvbox::Gruvbox::{BlueBright, Dark0, Dark0Hard, Dark1, Light2, YellowBright, YellowDim, OrangeDim, OrangeBright};


pub fn ui(f: &mut Frame, app: &mut App) {
    let border_style_selected = Style::default()
        .fg(YellowBright.into());
    let border_style_unselected = Style::default()
        .fg(YellowDim.into());
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
            Block::bordered().title("Nix builders list").title_bottom(
                "",
            )
            .title_style(
                if app.which_pane == WhichPane::Left {
                    title_style_selected
                } else {
                    title_style_unselected
                })
            .border_style(
                if app.which_pane == WhichPane::Left {
                    border_style_selected
                } else {
                    border_style_unselected
                })
            , // good for debugging
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
        .highlight_symbol("> ")
        ;
    f.render_stateful_widget(widget, chunks[0], &mut app.state);

    let mut table_state = TableState::default();
    let header = ["pid", "env", "parent pid", "p_mem", "v_mem", "runtime", "cmd"].into_iter().map(Cell::from).collect::<Row>();
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
            rows.push([
                &id.to_string(),
                &env.to_vec().join(" "),
                parent,
                &p_mem.to_string(),
                &v_mem.to_string(),
                &run_time.to_string(),
                &cmd.iter().take(8).cloned().collect::<Vec<_>>().join(" "),
            ].into_iter().map(|content| Cell::from(Text::from(content.to_string()))).collect::<Row>()
            )
        }
    }

    let widths = [
        Constraint::Percentage(4),
        Constraint::Percentage(0),
        Constraint::Percentage(4),
        Constraint::Percentage(4),
        Constraint::Percentage(4),
        Constraint::Percentage(4),
        Constraint::Percentage(80),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::bordered().title("BuilderInfo").title_bottom(" TAB - toggle all, j/k - up down, q to quit, ENTER - selectively open ").title_style(
                if app.which_pane == WhichPane::Right {
                    title_style_selected
                } else {
                    title_style_unselected
                })
            .border_style(
                if app.which_pane == WhichPane::Right {
                    border_style_selected
                } else {
                    border_style_unselected
                })
        )
        .highlight_style(
            Style::new()
        );
    f.render_stateful_widget(table, chunks[1], &mut table_state);
}
