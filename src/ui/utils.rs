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
    // Filter out -1.0 timeout sentinels before computing adjacent differences.
    // Without this, a timeout between two 1ms pings inflates jitter by ~2ms per
    // sentinel (|1.0 - (-1.0)| = 2.0, |-1.0 - 1.0| = 2.0).
    let valid: Vec<f64> = rtt.iter().copied().filter(|&r| r >= 0.0).collect();
    if valid.len() > 1 {
        let sum: f64 = valid.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
        sum / (valid.len() - 1) as f64
    } else {
        0.0
    }
}

pub fn calculate_p95(rtt: &VecDeque<f64>) -> f64 {
    let mut valid: Vec<f64> = rtt.iter().copied().filter(|&r| r >= 0.0).collect();
    if valid.is_empty() {
        return 0.0;
    }
    valid.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((valid.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(valid.len() - 1);
    valid[idx]
}

/// Convert an RTT float (ms) to a scaled u64 for sparkline display.
/// Multiplies by 100 to preserve 0.01 ms precision — without this, sub-ms
/// RTTs (e.g. 0.3 ms on LAN) would be truncated to 0 and appear as blank bars.
pub fn rtt_to_spark_unit(rtt: f64) -> u64 {
    if rtt < 0.0 { 0 } else { (rtt * 100.0).round() as u64 }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn deque(v: &[f64]) -> VecDeque<f64> {
        v.iter().copied().collect()
    }

    // ---- calculate_jitter ----

    #[test]
    fn jitter_ignores_timeout_sentinels() {
        // [1.0, -1.0, 2.0]: real jitter between 1.0→2.0 = 1.0 ms.
        // Before the fix, the sentinel inflated this to (2.0 + 3.0) / 2 = 2.5 ms.
        let j = calculate_jitter(&deque(&[1.0, -1.0, 2.0]));
        assert!((j - 1.0).abs() < 1e-9, "jitter={}", j);
    }

    #[test]
    fn jitter_all_sentinels_returns_zero() {
        assert_eq!(calculate_jitter(&deque(&[-1.0, -1.0, -1.0])), 0.0);
    }

    #[test]
    fn jitter_single_valid_returns_zero() {
        assert_eq!(calculate_jitter(&deque(&[-1.0, 5.0, -1.0])), 0.0);
    }

    #[test]
    fn jitter_no_sentinels_unchanged() {
        // [1.0, 3.0, 2.0] → diffs [2.0, 1.0] → jitter 1.5
        let j = calculate_jitter(&deque(&[1.0, 3.0, 2.0]));
        assert!((j - 1.5).abs() < 1e-9, "jitter={}", j);
    }

    // ---- rtt_to_spark_unit ----

    #[test]
    fn spark_unit_preserves_submillisecond() {
        assert_eq!(rtt_to_spark_unit(0.3), 30);  // 0.3 ms → 30 units
        assert_eq!(rtt_to_spark_unit(0.01), 1);  // 0.01 ms → 1 unit (not 0)
        assert_eq!(rtt_to_spark_unit(1.0), 100); // 1.0 ms → 100 units
        assert_eq!(rtt_to_spark_unit(10.5), 1050);
    }

    #[test]
    fn spark_unit_timeout_sentinel_is_blank() {
        assert_eq!(rtt_to_spark_unit(-1.0), 0);
        assert_eq!(rtt_to_spark_unit(-0.001), 0);
    }

    // ---- calculate_p95 ----

    #[test]
    fn p95_filters_sentinels() {
        // Only the three valid values should be considered.
        let p = calculate_p95(&deque(&[-1.0, 1.0, 2.0, 3.0, -1.0]));
        // 95th percentile of [1.0, 2.0, 3.0] = 3.0
        assert!((p - 3.0).abs() < 1e-9, "p95={}", p);
    }

    #[test]
    fn p95_all_sentinels_returns_zero() {
        assert_eq!(calculate_p95(&deque(&[-1.0, -1.0])), 0.0);
    }
}
