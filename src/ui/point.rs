use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Line, Span, Style};
use ratatui::style::Modifier;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::ip_data::IpData;
use crate::ui::theme::Theme;
use crate::ui::utils::{calculate_avg_rtt, calculate_jitter, calculate_loss_pkg, draw_errors_section};

pub fn draw_point_view(
    f: &mut Frame,
    ip_data: &[IpData],
    errs: &[String],
    area: Rect,
    theme: &Theme,
) {
    let ip_height: u16 = 5;
    let total_height = (ip_data.len() as u16) * ip_height + 2;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),               // legend
            Constraint::Length(total_height),    // ip blocks
            Constraint::Min(6),                  // errors
        ])
        .split(area);

    let legend = Line::from(vec![
        Span::styled(
            " Point View ",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("(", Style::default().fg(theme.dim)),
        Span::styled("●", Style::default().fg(theme.success)),
        Span::styled(" healthy  ", Style::default().fg(theme.dim)),
        Span::styled("▲", Style::default().fg(theme.warning)),
        Span::styled(" slow  ", Style::default().fg(theme.dim)),
        Span::styled("✗", Style::default().fg(theme.danger)),
        Span::styled(" timeout", Style::default().fg(theme.dim)),
        Span::styled(")", Style::default().fg(theme.dim)),
    ]);
    f.render_widget(Paragraph::new(legend), chunks[0]);

    let ip_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(ip_height); ip_data.len()])
        .margin(1)
        .split(chunks[1]);

    for (i, ip) in ip_data.iter().enumerate() {
        let avg_rtt = calculate_avg_rtt(&ip.rtts);
        let jitter = calculate_jitter(&ip.rtts);
        let loss_pkg = calculate_loss_pkg(ip.timeout, ip.received);
        let loss_color = theme.loss_color(loss_pkg);
        let plot_max = ip.max_rtt.max(1.0);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.border))
            .title(Line::from(vec![
                Span::styled(
                    format!(" ◆ {} ", ip.addr),
                    Style::default()
                        .fg(theme.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{} ", ip.ip), Style::default().fg(theme.dim)),
            ]));
        let inner = block.inner(ip_chunks[i]);
        f.render_widget(block, ip_chunks[i]);

        let inner_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);

        let last_str = if ip.last_attr == 0.0 {
            "< 0.01ms".to_string()
        } else if ip.last_attr == -1.0 {
            "timeout".to_string()
        } else {
            format!("{:.2}ms", ip.last_attr)
        };
        let last_color = if ip.last_attr == -1.0 {
            theme.danger
        } else {
            theme.rtt_color(ip.last_attr, plot_max)
        };

        let info = Line::from(vec![
            Span::styled("last ", Style::default().fg(theme.dim)),
            Span::styled(
                last_str,
                Style::default().fg(last_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  avg ", Style::default().fg(theme.dim)),
            Span::styled(format!("{:.2}ms", avg_rtt), Style::default().fg(theme.secondary)),
            Span::styled("  max ", Style::default().fg(theme.dim)),
            Span::styled(format!("{:.2}ms", ip.max_rtt), Style::default().fg(theme.warning)),
            Span::styled("  min ", Style::default().fg(theme.dim)),
            Span::styled(format!("{:.2}ms", ip.min_rtt), Style::default().fg(theme.success)),
            Span::styled("  jit ", Style::default().fg(theme.dim)),
            Span::styled(format!("{:.2}ms", jitter), Style::default().fg(theme.secondary)),
            Span::styled("  loss ", Style::default().fg(theme.dim)),
            Span::styled(
                format!("{:.2}%", loss_pkg),
                Style::default().fg(loss_color).add_modifier(Modifier::BOLD),
            ),
        ]);
        f.render_widget(Paragraph::new(info).wrap(Wrap { trim: true }), inner_chunks[0]);

        let mut points_spans: Vec<Span> = Vec::new();
        for &rtt in &ip.rtts {
            if rtt < 0.0 {
                points_spans.push(Span::styled("✗", Style::default().fg(theme.danger)));
            } else if rtt > plot_max * 0.8 {
                points_spans.push(Span::styled(
                    "▲",
                    Style::default().fg(theme.rtt_color(rtt, plot_max)),
                ));
            } else {
                points_spans.push(Span::styled(
                    "●",
                    Style::default().fg(theme.rtt_color(rtt, plot_max)),
                ));
            }
            points_spans.push(Span::raw(" "));
        }
        let points_para = Paragraph::new(Line::from(points_spans)).wrap(Wrap { trim: true });
        f.render_widget(points_para, inner_chunks[1]);
    }

    let errors_chunk = chunks.last().unwrap();
    draw_errors_section(f, errs, *errors_chunk, theme);
}
