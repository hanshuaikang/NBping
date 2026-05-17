use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Row, Table};
use ratatui::Frame;

use crate::ip_data::IpData;
use crate::ui::theme::Theme;
use crate::ui::utils::{calculate_avg_rtt, calculate_jitter, calculate_loss_pkg, draw_errors_section};

pub fn draw_table_view(
    f: &mut Frame,
    ip_data: &[IpData],
    errs: &[String],
    area: Rect,
    theme: &Theme,
) {
    // Pre-compute sort keys once so the comparator stays cheap and we
    // don't have to clone the RTT deques just to sort.
    let mut sortable: Vec<(usize, f64, f64)> = ip_data
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let loss = calculate_loss_pkg(d.timeout, d.received);
            let avg = calculate_avg_rtt(&d.rtts);
            (i, loss, avg)
        })
        .collect();

    sortable.sort_by(|&(_, la, aa), &(_, lb, ab)| {
        match la.partial_cmp(&lb) {
            Some(std::cmp::Ordering::Equal) => {
                aa.partial_cmp(&ab).unwrap_or(std::cmp::Ordering::Equal)
            }
            Some(ord) => ord,
            None => std::cmp::Ordering::Equal,
        }
    });

    let header_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    let header = Row::new(vec![
        "Rank", "Target", "IP", "Last", "Avg", "Max", "Min", "Jitter", "Loss",
    ])
    .style(header_style)
    .height(1);

    let n = sortable.len();
    let rows = sortable.iter().enumerate().map(|(index, &(orig, loss_pkg, avg_rtt))| {
        let d = &ip_data[orig];
        let jitter = calculate_jitter(&d.rtts);

        let rank = match index {
            0 => "🥇",
            1 => "🥈",
            2 => "🥉",
            _ if index + 1 == n && n > 3 => "🐢",
            _ => "▸",
        };

        let last_str = if d.last_attr == 0.0 {
            "< 0.01ms".to_string()
        } else if d.last_attr == -1.0 {
            "timeout".to_string()
        } else {
            format!("{:.2}ms", d.last_attr)
        };

        let row = Row::new(vec![
            rank.to_string(),
            d.addr.clone(),
            d.ip.clone(),
            last_str,
            format!("{:.2}ms", avg_rtt),
            format!("{:.2}ms", d.max_rtt),
            format!("{:.2}ms", d.min_rtt),
            format!("{:.2}ms", jitter),
            format!("{:.2}%", loss_pkg),
        ])
        .height(1);

        if loss_pkg > 50.0 {
            row.style(Style::default().fg(theme.danger).add_modifier(Modifier::BOLD))
        } else if loss_pkg > 0.0 {
            row.style(Style::default().fg(theme.warning))
        } else {
            row.style(Style::default().fg(theme.fg))
        }
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(5),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .style(Style::default().bg(theme.bg))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.border))
            .title(" Table — sorted by loss ↑ then latency ↑ "),
    )
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .highlight_symbol("▶ ");

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(6)].as_ref())
        .split(area);

    f.render_widget(table, chunks[0]);

    let errors_chunk = chunks.last().unwrap();
    draw_errors_section(f, errs, *errors_chunk, theme);
}
