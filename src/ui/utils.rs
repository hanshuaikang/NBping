use std::collections::VecDeque;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::prelude::{Line, Span, Style};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use crate::ui::theme::Theme;

pub fn calculate_avg_rtt(rtt: &VecDeque<f64>) -> f64 {
    if !rtt.is_empty() {
        let valid_rtt: Vec<f64> = rtt.iter().cloned().filter(|&rtt| rtt >= 0.0).collect();
        if !valid_rtt.is_empty() {
            let sum: f64 = valid_rtt.iter().sum();
            sum / valid_rtt.len() as f64
        } else {
            0.0
        }
    } else {
        0.0
    }
}

pub fn calculate_jitter(rtt: &VecDeque<f64>) -> f64 {
    if rtt.len() > 1 {
        let diffs: Vec<f64> = rtt.iter().zip(rtt.iter().skip(1)).map(|(y1, y2)| (y2 - y1).abs()).collect();
        let sum: f64 = diffs.iter().sum();
        sum / diffs.len() as f64
    } else {
        0.0
    }
}

pub fn calculate_loss_pkg(timeout: usize, received: usize) -> f64 {
    if timeout > 0 {
        (timeout as f64 / (received as f64 + timeout as f64)) * 100.0
    } else {
        0.0
    }
}

pub fn draw_errors_section(
    f: &mut Frame,
    errs: &[String],
    area: Rect,
    theme: &Theme,
) {
    if errs.is_empty() {
        return;
    }

    let recent_errors: Vec<Line> = errs
        .iter()
        .rev()
        .take(5)
        .map(|err| {
            Line::from(vec![
                Span::styled("⚠ ", Style::default().fg(theme.warning)),
                Span::styled(err.clone(), Style::default().fg(theme.danger)),
            ])
        })
        .collect();

    let errors_paragraph = Paragraph::new(recent_errors)
        .block(
            Block::default()
                .title(" 🚨 recent errors ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.danger)),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(errors_paragraph, area);
}
