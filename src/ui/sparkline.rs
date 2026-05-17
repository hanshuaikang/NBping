use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Line, Span, Style};
use ratatui::style::Modifier;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Sparkline, Wrap};
use ratatui::Frame;

use crate::ip_data::IpData;
use crate::ui::theme::Theme;
use crate::ui::utils::{calculate_avg_rtt, calculate_jitter, calculate_loss_pkg, calculate_p95, draw_errors_section};

pub fn draw_sparkline_view(
    f: &mut Frame,
    ip_data: &[IpData],
    errs: &[String],
    area: Rect,
    theme: &Theme,
) {
    let n = ip_data.len().max(1);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            std::iter::once(Constraint::Length(1))
                .chain(std::iter::repeat_n(Constraint::Length(5), n))
                .chain([Constraint::Min(6)])
                .collect::<Vec<_>>(),
        )
        .split(area);

    let legend = Line::from(vec![
        Span::styled(
            " Sparkline View ",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "(blank gap = timeout/error)",
            Style::default().fg(theme.dim),
        ),
    ]);
    f.render_widget(Paragraph::new(legend), chunks[0]);

    for (i, ip) in ip_data.iter().enumerate() {
        let avg_rtt = calculate_avg_rtt(&ip.rtts);
        let jitter = calculate_jitter(&ip.rtts);
        let p95 = calculate_p95(&ip.rtts);
        let loss_pkg = calculate_loss_pkg(ip.timeout, ip.received);
        let loss_color = theme.loss_color(loss_pkg);
        let plot_max = ip.max_rtt.max(1.0);

        let cell = chunks[i + 1];

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
        let inner = block.inner(cell);
        f.render_widget(block, cell);

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
            Span::styled("  p95 ", Style::default().fg(theme.dim)),
            Span::styled(format!("{:.2}ms", p95), Style::default().fg(theme.warning)),
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

        let spark_rect = inner_chunks[1];
        let width = spark_rect.width as usize;
        let rtts_len = ip.rtts.len();
        let skip = rtts_len.saturating_sub(width);
        let spark_data: Vec<u64> = ip
            .rtts
            .iter()
            .skip(skip)
            .map(|&rtt| if rtt < 0.0 { 0 } else { rtt as u64 })
            .collect();

        // Cap auto-scale at P95 so a single outlier (e.g. a one-off
        // 1200ms spike) doesn't pull every typical RTT down to level 0.
        // Values above the cap clip to a full bar, which highlights spikes.
        let spark_max = (p95 as u64).max(1);

        let spark = Sparkline::default()
            .data(&spark_data)
            .max(spark_max)
            .style(
                Style::default()
                    .fg(theme.rtt_color(avg_rtt, plot_max))
                    .bg(theme.bg),
            );
        f.render_widget(spark, spark_rect);
    }

    let errors_chunk = chunks.last().unwrap();
    draw_errors_section(f, errs, *errors_chunk, theme);
}
