use ratatui::style::Color;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ThemeKind {
    Day = 0,
    Night = 1,
}

const THEME_COUNT: u8 = 2;

// Semantic colors are theme-independent — green = healthy, yellow = high
// latency, red = packet loss. These never change regardless of which
// theme the user picks.
const SEMANTIC_SUCCESS: Color = Color::Rgb(46, 204, 113); // emerald green
const SEMANTIC_WARNING: Color = Color::Rgb(241, 196, 15); // sunflower yellow
const SEMANTIC_DANGER: Color = Color::Rgb(231, 76, 60); // alizarin red

// RTT-magnitude gradient — also theme-independent, mapping low → high
// latency from green to red.
const RTT_GRADIENT: [Color; 6] = [
    Color::Rgb(46, 204, 113),  // green
    Color::Rgb(130, 224, 80),  // lime
    Color::Rgb(241, 196, 15),  // yellow
    Color::Rgb(230, 126, 34),  // orange
    Color::Rgb(231, 76, 60),   // red
    Color::Rgb(192, 57, 43),   // deep red
];

impl ThemeKind {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => ThemeKind::Day,
            1 => ThemeKind::Night,
            _ => ThemeKind::Day,
        }
    }

    pub fn next(self) -> Self {
        Self::from_u8((self as u8 + 1) % THEME_COUNT)
    }

    pub fn name(self) -> &'static str {
        match self {
            ThemeKind::Day => "day",
            ThemeKind::Night => "night",
        }
    }

    pub fn palette(self) -> Theme {
        match self {
            // Day — light background. fg/bg both use Color::Reset so the
            // terminal's own colors show through (cream/white/etc.).
            ThemeKind::Day => Theme {
                bg: Color::Reset,
                fg: Color::Reset,
                primary: Color::Rgb(38, 139, 210),   // solarized blue
                secondary: Color::Rgb(42, 161, 152), // solarized cyan
                accent: Color::Rgb(108, 113, 196),   // solarized violet
                dim: Color::Rgb(147, 161, 161),      // solarized base1
                border: Color::Rgb(147, 161, 161),
                success: SEMANTIC_SUCCESS,
                warning: SEMANTIC_WARNING,
                danger: SEMANTIC_DANGER,
                gradient: RTT_GRADIENT,
            },

            // Night — explicit black background painted across the entire
            // frame. Off-white fg keeps everything readable.
            ThemeKind::Night => Theme {
                bg: Color::Rgb(0, 0, 0),
                fg: Color::Rgb(220, 220, 220),
                primary: Color::Rgb(125, 207, 255),  // tokyo cyan
                secondary: Color::Rgb(122, 162, 247), // tokyo blue
                accent: Color::Rgb(187, 154, 247),   // tokyo magenta
                dim: Color::Rgb(120, 130, 160),
                border: Color::Rgb(90, 100, 130),
                success: SEMANTIC_SUCCESS,
                warning: SEMANTIC_WARNING,
                danger: SEMANTIC_DANGER,
                gradient: RTT_GRADIENT,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub primary: Color,
    pub secondary: Color,
    pub accent: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub dim: Color,
    pub border: Color,
    pub gradient: [Color; 6],
}

impl Theme {
    pub fn rtt_color(&self, rtt: f64, max_rtt: f64) -> Color {
        if rtt < 0.0 {
            return self.danger;
        }
        if max_rtt <= 0.0 {
            return self.gradient[0];
        }
        let ratio = (rtt / max_rtt).clamp(0.0, 1.0);
        let idx = ((ratio * (self.gradient.len() - 1) as f64).round() as usize)
            .min(self.gradient.len() - 1);
        self.gradient[idx]
    }

    pub fn loss_color(&self, loss: f64) -> Color {
        if loss > 50.0 {
            self.danger
        } else if loss > 0.0 {
            self.warning
        } else {
            self.success
        }
    }
}

pub fn load_theme(slot: &AtomicU8) -> Theme {
    ThemeKind::from_u8(slot.load(Ordering::Relaxed)).palette()
}

pub fn load_theme_kind(slot: &AtomicU8) -> ThemeKind {
    ThemeKind::from_u8(slot.load(Ordering::Relaxed))
}

pub fn store_theme(slot: &AtomicU8, kind: ThemeKind) {
    slot.store(kind as u8, Ordering::Relaxed);
}
