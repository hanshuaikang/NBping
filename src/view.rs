use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum View {
    Graph = 0,
    Table = 1,
    Point = 2,
    Sparkline = 3,
}

impl View {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "graph" => Some(View::Graph),
            "table" => Some(View::Table),
            "point" => Some(View::Point),
            "sparkline" => Some(View::Sparkline),
            _ => None,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => View::Graph,
            1 => View::Table,
            2 => View::Point,
            3 => View::Sparkline,
            _ => View::Graph,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            View::Graph => "graph",
            View::Table => "table",
            View::Point => "point",
            View::Sparkline => "sparkline",
        }
    }

    pub fn next(self) -> Self {
        Self::from_u8((self as u8 + 1) % 4)
    }
}

pub fn load_view(slot: &AtomicU8) -> View {
    View::from_u8(slot.load(Ordering::Relaxed))
}

pub fn store_view(slot: &AtomicU8, view: View) {
    slot.store(view as u8, Ordering::Relaxed);
}
