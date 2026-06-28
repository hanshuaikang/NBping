use std::error::Error;
use std::io::{self, Stdout};
use std::sync::atomic::AtomicU8;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event;
use ratatui::crossterm::event::{Event, KeyCode, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::style::Style;
use ratatui::widgets::Block;
use ratatui::Terminal;

use crate::ip_data::IpData;
use crate::ui::theme::{load_theme, load_theme_kind, store_theme, Theme};
use crate::ui::{
    draw_graph_view, draw_layout, draw_point_view, draw_sparkline_view, draw_table_view,
    LayoutContext,
};
use crate::view::{load_view, store_view, View};

pub fn init_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>, Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

pub fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
) -> Result<(), Box<dyn Error>> {
    disable_raw_mode()?;
    terminal.show_cursor()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

pub fn draw_interface<B: Backend>(
    terminal: &mut Terminal<B>,
    view: View,
    theme: &Theme,
    theme_kind: crate::ui::theme::ThemeKind,
    ip_data: &[IpData],
    errs: &[String],
    tick: u64,
) -> Result<(), Box<dyn Error>> {
    terminal.draw(|f| {
        // Paint the theme background across the whole frame first so themes
        // like night get an explicit black canvas instead of inheriting the
        // terminal's bg. Subsequent widgets render on top without resetting
        // bg (their Spans only set fg).
        f.render_widget(
            Block::default().style(Style::default().bg(theme.bg)),
            f.area(),
        );

        let ctx = LayoutContext {
            view,
            theme: *theme,
            theme_kind,
            ip_data,
            tick,
        };
        let body = draw_layout(f, f.area(), &ctx);
        match view {
            View::Graph => draw_graph_view(f, ip_data, errs, body, theme),
            View::Table => draw_table_view(f, ip_data, errs, body, theme),
            View::Point => draw_point_view(f, ip_data, errs, body, theme),
            View::Sparkline => draw_sparkline_view(f, ip_data, errs, body, theme),
        }
    })?;
    Ok(())
}

pub fn draw_interface_with_updates<B: Backend>(
    terminal: &mut Terminal<B>,
    view_slot: Arc<AtomicU8>,
    theme_slot: Arc<AtomicU8>,
    ip_data: &Arc<Mutex<Vec<IpData>>>,
    ping_update_rx: mpsc::Receiver<IpData>,
    running: Arc<Mutex<bool>>,
    errs: Arc<Mutex<Vec<String>>>,
    output_file: Option<String>,
) -> Result<(), Box<dyn Error>> {
    let mut output_file_handle = if let Some(ref output_path) = output_file {
        // create_new atomically fails if the file already exists, eliminating
        // the TOCTOU window between the exists() check in main() and the open.
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(output_path)
        {
            Ok(file) => Some(file),
            Err(e) => {
                let mut errs = errs.lock().unwrap();
                errs.push(format!("Failed to create output file: {}", e));
                None
            }
        }
    } else {
        None
    };

    let mut tick: u64 = 0;
    let mut last_tick = std::time::Instant::now();
    let tick_rate = Duration::from_millis(100);

    let render =
        |terminal: &mut Terminal<B>,
         tick: u64,
         errs: &Arc<Mutex<Vec<String>>>,
         ip_data: &Arc<Mutex<Vec<IpData>>>,
         view_slot: &Arc<AtomicU8>,
         theme_slot: &Arc<AtomicU8>|
         -> Result<(), Box<dyn Error>> {
            let view = load_view(view_slot);
            let theme = load_theme(theme_slot);
            let theme_kind = load_theme_kind(theme_slot);
            let ip_data = ip_data.lock().unwrap();
            let errs = errs.lock().unwrap();
            draw_interface(terminal, view, &theme, theme_kind, &ip_data, &errs, tick)
        };

    // Initial paint
    render(terminal, tick, &errs, ip_data, &view_slot, &theme_slot).ok();

    loop {
        if !*running.lock().unwrap() {
            break Ok(());
        }

        let mut dirty = false;

        if event::poll(Duration::from_millis(50)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        *running.lock().unwrap() = false;
                        break Ok(());
                    }
                    KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                        *running.lock().unwrap() = false;
                        break Ok(());
                    }
                    KeyCode::Char('1') => {
                        store_view(&view_slot, View::Graph);
                        dirty = true;
                    }
                    KeyCode::Char('2') => {
                        store_view(&view_slot, View::Table);
                        dirty = true;
                    }
                    KeyCode::Char('3') => {
                        store_view(&view_slot, View::Point);
                        dirty = true;
                    }
                    KeyCode::Char('4') => {
                        store_view(&view_slot, View::Sparkline);
                        dirty = true;
                    }
                    KeyCode::Tab => {
                        store_view(&view_slot, load_view(&view_slot).next());
                        dirty = true;
                    }
                    KeyCode::Char('t') | KeyCode::Char('T') => {
                        store_theme(&theme_slot, load_theme_kind(&theme_slot).next());
                        dirty = true;
                    }
                    _ => {}
                }
            }
        }

        let remaining = tick_rate.saturating_sub(last_tick.elapsed());
        if let Ok(updated_data) = ping_update_rx.recv_timeout(remaining) {
            {
                let mut ip_data_guard = ip_data.lock().unwrap();
                let last_attr = updated_data.last_attr;
                let addr = updated_data.addr.clone();
                let ip = updated_data.ip.clone();

                if let Some(pos) = ip_data_guard
                    .iter()
                    .position(|d| d.addr == updated_data.addr && d.ip == updated_data.ip)
                {
                    ip_data_guard[pos] = updated_data;
                }

                if let Some(ref mut file) = output_file_handle {
                    use std::io::Write;
                    let latency_str = if last_attr == -1.0 {
                        "timeout".to_string()
                    } else {
                        format!("{:.2}ms", last_attr)
                    };
                    if let Err(e) = writeln!(file, "{} {} {}", addr, ip, latency_str) {
                        let mut errs = errs.lock().unwrap();
                        errs.push(format!("Failed to write to output file: {}", e));
                    }
                }
            }
            dirty = true;
        }

        if last_tick.elapsed() >= tick_rate {
            tick = tick.wrapping_add(1);
            last_tick = std::time::Instant::now();
            dirty = true;
        }

        if dirty {
            render(terminal, tick, &errs, ip_data, &view_slot, &theme_slot).ok();
        }
    }
}
