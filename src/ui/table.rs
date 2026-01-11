use ratatui::backend::Backend;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Modifier, Style};
use ratatui::widgets::{Block, Paragraph, Row, Table};
use crate::ip_data::IpData;
use crate::ui::utils::{calculate_avg_rtt, calculate_jitter, calculate_loss_pkg, draw_errors_section};


pub fn draw_table_view<B: Backend>(
    f: &mut Frame,
    ip_data: &[IpData],
    errs: &[String],
    area: Rect,
) {
    let mut data = ip_data.to_vec();

    data.sort_by(|a, b| {
        let loss_a = calculate_loss_pkg(a.timeout, a.received);
        let loss_b = calculate_loss_pkg(b.timeout, b.received);

        // sort by loss rate first, then by latency
        match loss_a.partial_cmp(&loss_b) {
            Some(std::cmp::Ordering::Equal) => {
                let avg_a = calculate_avg_rtt(&a.rtts);
                let avg_b = calculate_avg_rtt(&b.rtts);
                avg_a.partial_cmp(&avg_b).unwrap_or(std::cmp::Ordering::Equal)
            }
            Some(ordering) => ordering,
            None => std::cmp::Ordering::Equal
        }
    });


    let header_style = Style::default()
        .add_modifier(Modifier::BOLD);

    let selected_style = Style::default()
        .add_modifier(Modifier::REVERSED);

    // create header
    let header = Row::new(vec![
        "Rank",
        "Target",
        "Ip",
        "Last Rtt",
        "Avg Rtt",
        "Max",
        "Min",
        "Jitter",
        "Loss",
    ])
        .style(header_style)
        .height(1);


    // create rows
    let rows = data.iter().enumerate().map(|(index, data)| {
        let avg_rtt = calculate_avg_rtt(&data.rtts);
        let jitter = calculate_jitter(&data.rtts);
        let loss_pkg = calculate_loss_pkg(data.timeout, data.received);

        let rank = match index {
            0 => "🥇".to_string(),
            1 => "🥈".to_string(),
            2 => "🥉".to_string(),
            n if n < 10 && n != ip_data.len() - 1 => "🏆".to_string(),
            _ => "🐢".to_string(),
        };

        let row = Row::new(vec![
            rank,
            data.addr.clone(),
            data.ip.clone(),
            if data.last_attr == 0.0 {
                "< 0.01ms".to_string()
            } else if data.last_attr == -1.0 {
                "0.0ms".to_string()
            } else {
                format!("{:.2}ms", data.last_attr)
            },
            format!("{:.2}ms", avg_rtt),
            format!("{:.2}ms", data.max_rtt),
            format!("{:.2}ms", data.min_rtt),
            format!("{:.2}ms", jitter),
            format!("{:.2}%", loss_pkg),
        ]).height(1);

        // highlight the row with different colors
        if loss_pkg > 50.0 {
            row.style(Style::default().bg(Color::Red).fg(Color::White)) // 淡红色
        } else if loss_pkg > 0.0 {
            row.style(Style::default().bg(Color::Yellow).fg(Color::White)) // 淡黄色
        } else {
            row
        }
    });


    let table = Table::new(
        rows,
        [
            Constraint::Percentage(3),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
            Constraint::Percentage(10),
        ],
    )
        .header(header)
        .block(Block::default()
            .title("🏎  NBping Table (Sort by: Loss Rate ↑ then Latency ↑)"))
        .row_highlight_style(selected_style)
        .highlight_symbol(">> ");

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(6),
        ].as_ref())
        .split(area);

    // black line
    let blank = Paragraph::new("");
    f.render_widget(blank, chunks[0]);
    f.render_widget(table, chunks[1]);

    let errors_chunk = chunks.last().unwrap();
    draw_errors_section::<B>(f, errs, *errors_chunk);
}