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

use crate::App;
use crate::{
    get_stats::{gen_tree, get_active_users_and_pids, ProcMetadata},
    gruvbox::Gruvbox::{Dark0Hard, Dark0Soft, Light0Soft},
};

pub fn ui(f: &mut Frame, app: &mut App) {
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
                "TAB - toggle all, j/k - up down, esc/q to quit, ENTER - selectively open ",
            ), // good for debugging
               // .title_bottom(format!("{:?}", app.state)),
        )
        .highlight_style(
            Style::new()
                .fg(Color::Black)
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    f.render_stateful_widget(widget, chunks[0], &mut app.state);

    let mut table_state = TableState::default();
    let header = ["pid", "env", "parent pid", "physical mem", "virtual mem", "run time", "cmd"].into_iter().map(Cell::from).collect::<Row>();
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
                &env.into_iter().cloned().collect::<Vec<_>>().join(" "),
                parent,
                &p_mem.to_string(),
                &v_mem.to_string(),
                &run_time.to_string(),
                &cmd.into_iter().take(3).cloned().collect::<Vec<_>>().join(" "),
            ].into_iter().map(|content| Cell::from(Text::from(format!("{content}")))).collect::<Row>()
            )
        }
    }

    let widths = [
        Constraint::Percentage(14),
        Constraint::Percentage(14),
        Constraint::Percentage(14),
        Constraint::Percentage(14),
        Constraint::Percentage(14),
        Constraint::Percentage(14),
        Constraint::Percentage(16),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::new().title("BuilderInfo"))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(table, chunks[1], &mut table_state);
}
