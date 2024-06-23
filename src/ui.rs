use ratatui::{layout::{Alignment, Constraint, Layout, Margin}, style::{Color, Modifier, Style, Stylize}, symbols::scrollbar, widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation}};
use ratatui::text::{Line, Masked, Span};
use ratatui::Frame;
use tui_tree_widget::Tree;

use crate::{get_stats::{gen_tree, get_active_users_and_pids}, gruvbox::Gruvbox::{Dark0Hard, Dark0Soft, Light0Soft}};
use crate::App;

// pub fn ui(f: &mut Frame, app: &mut App) {
    // let bg: Color = Dark0Soft.into();
    // let fg: Color = Light0Soft.into();
    // let size = f.size();
    // let style = Style::new().bg(bg).fg(fg);
    //
    // // Words made "loooong" to demonstrate line breaking.
    // let s = "Veeeeeeeeeeeeeeeery    loooooooooooooooooong   striiiiiiiiiiiiiiiiiiiiiiiiiing.   ";
    // let mut long_line = s.repeat(usize::from(size.width) / s.len() + 4);
    // long_line.push('\n');
    //
    // let chunks = Layout::vertical([
    //     // title
    //     Constraint::Min(1),
    //     //content
    //     Constraint::Percentage(100),
    // ])
    // .split(size);
    //
    // let text = vec![
    //     Line::from("This is a line "),
    //     Line::from("This is a line   ".red()),
    //     Line::from("This is a line".on_dark_gray()),
    //     Line::from("This is a longer line".crossed_out()),
    //     Line::from(long_line.clone()),
    //     Line::from("This is a line".reset()),
    //     Line::from(vec![
    //         Span::raw("Masked text: "),
    //         Span::styled(Masked::new("password", '*'), Style::new().fg(Color::Red)),
    //     ]),
    //     Line::from("This is a line "),
    //     Line::from("This is a line   ".red()),
    //     Line::from("This is a line".on_dark_gray()),
    //     Line::from("This is a longer line".crossed_out()),
    //     Line::from(long_line.clone()),
    //     Line::from("This is a line".reset()),
    //     Line::from(vec![
    //         Span::raw("Masked text: "),
    //         Span::styled(Masked::new("password", '*'), Style::new().fg(Color::Red)),
    //     ]),
    // ];
    // app.vertical_scroll_state = app.vertical_scroll_state.content_length(text.len());
    // app.horizontal_scroll_state = app.horizontal_scroll_state.content_length(long_line.len());
    //
    // let create_block = |title: &'static str| Block::bordered().gray().title(title.bold());
    //
    // let title = Block::new()
    //     .title_alignment(Alignment::Center)
    //     .title("Use h j k l or ◄ ▲ ▼ ► to scroll ".bold())
    //     .style(style);
    // f.render_widget(title, chunks[0]);
    //
    // let paragraph = Paragraph::new(text.clone())
    //     .gray()
    //     .block(create_block("Vertical scrollbar with arrows"))
    //     .style(style)
    //     .scroll((app.vertical_scroll as u16, app.horizontal_scroll as u16));
    //
    // f.render_widget(paragraph, chunks[1]);
    //
    // f.render_stateful_widget(
    //     Scrollbar::new(ScrollbarOrientation::VerticalRight)
    //         .begin_symbol(Some("↑"))
    //         .end_symbol(Some("↓"))
    //         .track_style(style)
    //         .thumb_style(style)
    //         ,
    //     chunks[1],
    //     &mut app.vertical_scroll_state,
    // );
    //
    // f.render_stateful_widget(
    //     Scrollbar::new(ScrollbarOrientation::HorizontalBottom)
    //         .begin_symbol(Some("←"))
    //         .end_symbol(Some("→"))
    //         .track_style(style)
    //         .thumb_style(style)
    //         ,
    //     chunks[1],
    //     &mut app.horizontal_scroll_state,
    // );
    //
// }
//

pub fn ui(f: &mut Frame, app: &mut App) {
    let user_map = get_active_users_and_pids();
    let items = gen_tree(&user_map);
    let area = f.size();
    let widget = Tree::new(&items)
        .expect("all item identifiers are unique")
        .block(
            Block::bordered()
            .title("Nix builders list")
            .title_bottom("TAB - toggle all, j/k - up down, esc/q to quit, ENTER - selectively open ")
            // good for debugging
            // .title_bottom(format!("{:?}", app.state)),
        )
        // .experimental_scrollbar(Some(
        //         Scrollbar::new(ScrollbarOrientation::VerticalRight)
        //         .begin_symbol(None)
        //         .track_symbol(None)
        //         .end_symbol(None),
        // ))
        .highlight_style(
            Style::new()
            .fg(Color::Black)
            .bg(Color::LightGreen)
            .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    f.render_stateful_widget(widget, area, &mut app.state);
}
