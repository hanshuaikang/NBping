use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Span, Style};
use ratatui::style::Modifier;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::ip_data::IpData;
use crate::ui::theme::{Theme, ThemeKind};
use crate::ui::utils::{calculate_avg_rtt, calculate_loss_pkg};
use crate::view::View;

const HEARTBEAT: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BLOCKS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
const METRICS_WIDTH: u16 = 58;

// Indigo palette (Tailwind 400→900). Theme-independent. Mid-range shades
// stay legible against both white and black terminal backgrounds; the
// cycling across all 6 shades creates the flowing "ribbon" effect.
const WAVE_PALETTE: [Color; 6] = [
    Color::Rgb(49, 46, 129),   // indigo-900
    Color::Rgb(67, 56, 202),   // indigo-700
    Color::Rgb(79, 70, 229),   // indigo-600
    Color::Rgb(99, 102, 241),  // indigo-500
    Color::Rgb(129, 140, 248), // indigo-400
    Color::Rgb(165, 180, 252), // indigo-300
];

pub struct LayoutContext<'a> {
    pub view: View,
    pub theme: Theme,
    pub theme_kind: ThemeKind,
    pub ip_data: &'a [IpData],
    pub tick: u64,
}

pub fn draw_layout(f: &mut Frame, area: Rect, ctx: &LayoutContext) -> Rect {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(5),    // body
            Constraint::Length(1), // status bar
        ])
        .split(area);

    render_header(f, chunks[0], ctx);
    render_status(f, chunks[2], ctx);

    chunks[1]
}

fn render_header(f: &mut Frame, area: Rect, ctx: &LayoutContext) {
    let theme = ctx.theme;

    let total_targets = ctx.ip_data.len();
    let (total_recv, total_timeout, sum_avg, alive) = ctx
        .ip_data
        .iter()
        .fold((0usize, 0usize, 0.0f64, 0usize), |(r, t, s, a), d| {
            let avg = calculate_avg_rtt(&d.rtts);
            let alive = if d.received > 0 { a + 1 } else { a };
            (r + d.received, t + d.timeout, s + avg, alive)
        });
    // Divide by alive (targets with at least one successful ping), not total_targets.
    // Dead/unreachable targets contribute 0.0 to sum_avg, so including them in the
    // denominator makes the header average misleadingly low.
    let global_avg = if alive > 0 { sum_avg / alive as f64 } else { 0.0 };
    let global_loss = calculate_loss_pkg(total_timeout, total_recv);
    let beat = HEARTBEAT[(ctx.tick as usize) % HEARTBEAT.len()];

    let title = Line::from(vec![
        Span::styled(
            " 🏎  NBping ",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("· {} ", ctx.view.name()),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            beat,
            Style::default()
                .fg(theme.secondary)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let metrics = Line::from(vec![
        Span::styled("targets ", Style::default().fg(theme.dim)),
        Span::styled(
            format!("{}/{}", alive, total_targets),
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled("sent ", Style::default().fg(theme.dim)),
        Span::styled(
            format!("{}", total_recv + total_timeout),
            Style::default().fg(theme.fg),
        ),
        Span::raw("   "),
        Span::styled("avg ", Style::default().fg(theme.dim)),
        Span::styled(
            format!("{:.2}ms", global_avg),
            Style::default().fg(theme.secondary),
        ),
        Span::raw("   "),
        Span::styled("loss ", Style::default().fg(theme.dim)),
        Span::styled(
            format!("{:.2}%", global_loss),
            Style::default().fg(theme.loss_color(global_loss)),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .title(title)
        .title_alignment(Alignment::Left);

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split inner: activity bar on the left, metrics right-aligned on the right.
    // On terminals narrow enough that metrics already fill the row, the bar
    // collapses to zero width.
    let metrics_width = METRICS_WIDTH.min(inner.width);
    let bar_width = inner.width.saturating_sub(metrics_width);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(bar_width), Constraint::Length(metrics_width)])
        .split(inner);

    render_activity_bar(f, cols[0], ctx);

    let para = Paragraph::new(metrics).alignment(Alignment::Right);
    f.render_widget(para, cols[1]);
}

fn render_activity_bar(f: &mut Frame, area: Rect, ctx: &LayoutContext) {
    if area.width < 4 {
        return;
    }
    let t = ctx.tick as f64;
    let n_grad = WAVE_PALETTE.len();
    let max_level = (BLOCKS.len() - 1) as f64;

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(area.width as usize);
    spans.push(Span::raw(" "));

    let body = area.width.saturating_sub(2) as usize;
    for i in 0..body {
        let x = i as f64;
        // Two overlaid sine waves drive height. No data input — purely
        // decorative. Phase advances with tick (~10fps); the crest sweeps
        // left→right across the bar.
        let w1 = (x * 0.30 - t * 0.45).sin();
        let w2 = (x * 0.12 + t * 0.20).sin() * 0.5;
        let amp = ((w1 + w2 + 1.5) / 3.0).clamp(0.0, 1.0);
        let level = ((amp * max_level).round() as usize).min(BLOCKS.len() - 1);

        // Indigo shade cycles by position + tick — independent flow speed
        // from the height wave to give a layered ribbon effect.
        let hue = (i as i64 + (t * 0.35) as i64).rem_euclid(n_grad as i64) as usize;
        let color = WAVE_PALETTE[hue];

        spans.push(Span::styled(BLOCKS[level], Style::default().fg(color)));
    }

    spans.push(Span::raw(" "));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_status(f: &mut Frame, area: Rect, ctx: &LayoutContext) {
    let theme = ctx.theme;

    let key = |k: &'static str, label: &'static str| -> Vec<Span<'static>> {
        vec![
            Span::styled(
                k,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(label, Style::default().fg(theme.dim)),
            Span::raw("  "),
        ]
    };

    let mut spans: Vec<Span> = Vec::new();
    spans.extend(key(" 1 ", "graph"));
    spans.extend(key(" 2 ", "table"));
    spans.extend(key(" 3 ", "point"));
    spans.extend(key(" 4 ", "sparkline"));
    spans.extend(key(" Tab ", "next"));
    spans.extend(key(" t ", "theme"));
    spans.extend(key(" q ", "quit"));

    spans.push(Span::styled(
        format!(" view:{}  theme:{} ", ctx.view.name(), ctx.theme_kind.name()),
        Style::default()
            .fg(theme.primary)
            .add_modifier(Modifier::REVERSED),
    ));

    let line = Line::from(spans);
    let para = Paragraph::new(line);
    f.render_widget(para, area);
}
