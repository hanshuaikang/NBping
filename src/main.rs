mod network;
mod draw;
mod terminal;
mod ip_data;
mod ui;
mod ping_event;
mod data_processor;
mod exporter;
mod view;
mod config;

use clap::{Parser, Subcommand};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use tokio::{task, runtime::Builder, signal};
use crate::ip_data::IpData;
use crate::ping_event::PingEvent;
use crate::data_processor::start_data_processor;
use std::sync::mpsc;
use crate::network::send_ping;
use crate::exporter::{PrometheusMetrics, http_server, spawn_ping_workers};
use crate::view::View;
use crate::config::{FileConfig, Mode};

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> std::io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

#[derive(Parser, Debug)]
#[command(
    version = "v0.7.1",
    author = "hanshuaikang<https://github.com/hanshuaikang>",
    about = "🏎  NBping mean NB Ping, A Ping Tool in Rust with Real-Time Data and Visualizations"
)]
struct Args {
    /// Target IP address or hostname to ping
    #[arg(help = "target IP address or hostname to ping", required = false)]
    target: Vec<String>,

    /// Path to a YAML config file (CLI flags override values from this file)
    #[arg(long, global = true, help = "Path to a YAML config file (CLI flags override its values)")]
    config: Option<String>,

    /// Number of pings to send, when count is 0, the maximum number of pings per address is calculated
    #[arg(short, long, help = "Number of pings to send [default: 0 = unlimited]")]
    count: Option<usize>,

    /// Interval in seconds between pings
    #[arg(short, long, help = "Interval in seconds between pings [default: 0]")]
    interval: Option<i32>,

    #[clap(long = "force_ipv6", default_value_t = false, short = '6', global = true, help = "Force using IPv6 (config-only field can also enable this)")]
    pub force_ipv6: bool,

    #[arg(
        short = 'm',
        long,
        help = "Specify the maximum number of target addresses, Only works on one target address [default: 0]"
    )]
    multiple: Option<i32>,

    #[arg(short, long, help = "Initial view mode: graph/table/point/sparkline (switch at runtime with 1-4 / Tab) [default: graph]")]
    view_type: Option<String>,

    #[arg(short = 'o', long = "output", help = "Output file to save ping results")]
    output: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Exporter mode for monitoring
    Exporter {
        /// Target IP addresses or hostnames to ping
        #[arg(help = "target IP addresses or hostnames to ping", required = false)]
        target: Vec<String>,

        /// Interval in seconds between pings
        #[arg(short, long, help = "Interval in seconds between pings [default: 1]")]
        interval: Option<i32>,

        /// Prometheus metrics HTTP port
        #[arg(short, long, help = "Prometheus metrics HTTP port [default: 9090]")]
        port: Option<u16>,
    },
}


fn main() -> Result<(), Box<dyn std::error::Error>> {
    // parse command line arguments
    let args = Args::parse();

    // Load the YAML config file when one was provided. Values from it act as a
    // baseline that command-line flags override (CLI > YAML > built-in default).
    let file_cfg = match &args.config {
        Some(path) => match FileConfig::load(path) {
            Ok(cfg) => cfg,
            Err(err) => {
                eprintln!("Error: {:#}", err);
                std::process::exit(1);
            }
        },
        None => FileConfig::default(),
    };

    // Decide which mode to run:
    //  - an explicit `exporter` subcommand always wins
    //  - otherwise the config file's `mode` field decides (defaulting to tui)
    let run_exporter = match &args.command {
        Some(Commands::Exporter { .. }) => true,
        None => match file_cfg.mode() {
            Ok(mode) => mode == Mode::Exporter,
            Err(err) => {
                eprintln!("Error: {:#}", err);
                std::process::exit(1);
            }
        },
    };

    if run_exporter {
        // Exporter mode is reachable two ways: the `exporter` subcommand (with its
        // own target/interval/port) or top-level flags alongside `--config mode: exporter`.
        let (sub_target, sub_interval, sub_port) = match args.command {
            Some(Commands::Exporter { target, interval, port }) => (target, interval, port),
            None => (Vec::new(), None, None),
        };
        let cfg = config::resolve_exporter(
            sub_target,
            sub_interval,
            sub_port,
            args.target,
            args.interval,
            args.force_ipv6,
            &file_cfg,
        );
        if cfg.targets.is_empty() {
            eprintln!("Error: at least one target is required (via CLI or config 'targets')");
            std::process::exit(1);
        }

        let worker_threads = (cfg.targets.len() + 1).max(1);
        let rt = Builder::new_multi_thread()
            .worker_threads(worker_threads)
            .enable_all()
            .build()?;

        let res = rt.block_on(run_exporter_mode(cfg.targets, cfg.interval, cfg.port, cfg.force_ipv6));
        if let Err(err) = res {
            eprintln!("{}", err);
            std::process::exit(1);
        }
    } else {
        // Default TUI ping mode. Resolve every value as CLI > YAML > default.
        let cfg = config::resolve_tui(
            args.target,
            args.count,
            args.interval,
            args.force_ipv6,
            args.multiple,
            args.view_type,
            args.output,
            &file_cfg,
        );
        if cfg.targets.is_empty() {
            eprintln!("Error: at least one target is required (via CLI or config 'targets')");
            std::process::exit(1);
        }
        // YAML interval is validated in config.rs; guard the CLI path here.
        if cfg.interval < 0 {
            eprintln!("Error: interval must be >= 0, got {}", cfg.interval);
            std::process::exit(1);
        }

        // set Ctrl+C and q and esc to exit
        let running = Arc::new(Mutex::new(true));

        // check output file
        if let Some(ref output_path) = cfg.output {
            if std::path::Path::new(output_path).exists() {
                eprintln!("Output file already exists: {}", output_path);
                std::process::exit(1);
            }
        }

        // Calculate worker threads based on IP count
        let ip_count = if cfg.targets.len() == 1 && cfg.multiple > 0 {
            cfg.multiple as usize
        } else {
            cfg.targets.len()
        };
        let worker_threads = (ip_count + 1).max(1);

        // Create tokio runtime with specific worker thread count
        let rt = Builder::new_multi_thread()
            .worker_threads(worker_threads)
            .enable_all()
            .build()?;

        let res = rt.block_on(run_app(cfg.targets, cfg.count, cfg.interval, running.clone(), cfg.force_ipv6, cfg.multiple, cfg.view_type, cfg.output));
        if let Err(err) = res {
            eprintln!("{}", err);
            std::process::exit(1);
        }
    }
    Ok(())
}

async fn run_app(
    targets: Vec<String>,
    count: usize,
    interval: i32,
    running: Arc<Mutex<bool>>,
    force_ipv6: bool,
    multiple: i32,
    view_type: String,
    output_file: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {

    // init terminal
    let terminal = draw::init_terminal()?;
    let terminal_guard = Arc::new(Mutex::new(terminal::TerminalGuard::new(terminal)));


    // ping event channel (network -> data processor)
    let (ping_event_tx, ping_event_rx) = mpsc::sync_channel::<PingEvent>(0);

    // ui data channel (data processor -> ui)
    let (ui_data_tx, ui_data_rx) = mpsc::sync_channel::<IpData>(0);

    let ping_event_tx = Arc::new(ping_event_tx);


    let mut ips = Vec::new();
    // if multiple is set, get multiple IP addresses for each target
    if targets.len() == 1 && multiple > 0 {
        // get multiple IP addresses for the target
        ips = network::get_multiple_host_ipaddr(&targets[0], force_ipv6, multiple as usize)?;
    } else {
        // get IP address for each target
        for target in &targets {
            let ip = network::get_host_ipaddr(target, force_ipv6)?;
            ips.push(ip);
        }
    }

    // Define initial data for UI
    let ip_data = Arc::new(Mutex::new(ips.iter().enumerate().map(|(i, _)| IpData {
        ip: String::new(),
        addr: if targets.len() == 1 { targets[0].clone() } else { targets[i].clone() },
        rtts: VecDeque::new(),
        last_attr: 0.0,
        min_rtt: 0.0,
        max_rtt: 0.0,
        timeout: 0,
        received: 0,
        pop_count: 0,
    }).collect::<Vec<_>>()));

    // Start data processor
    let targets_for_processor: Vec<(String, String)> = ips.iter().enumerate().map(|(i, ip)| {
        let addr = if targets.len() == 1 { targets[0].clone() } else { targets[i].clone() };
        (addr, ip.clone())
    }).collect();

    start_data_processor(
        ping_event_rx,
        ui_data_tx,
        targets_for_processor,
        running.clone(),
    );

    let initial_view = View::from_str(&view_type).unwrap_or(View::Graph);
    let view_slot = Arc::new(AtomicU8::new(initial_view as u8));
    let theme_slot = Arc::new(AtomicU8::new(0));

    let errs = Arc::new(Mutex::new(Vec::new()));

    // saturating_mul prevents i32 overflow for very large interval values.
    let interval = if interval == 0 { 500 } else { interval.saturating_mul(1000) };
    let mut tasks = Vec::new();

    for (i, ip) in ips.iter().enumerate() {
        let ip = ip.clone();
        let running = running.clone();
        let errs = errs.clone();
        let task = task::spawn({
            let errs = errs.clone();
            let ping_event_tx = ping_event_tx.clone();
            let ip_data = ip_data.clone();
            let mut data = ip_data.lock().unwrap();
            // update the ip
            data[i].ip = ip.clone();
            let addr = data[i].addr.clone();
            async move {
                send_ping(addr, ip, errs.clone(), count, interval, running.clone(), ping_event_tx).await.unwrap();
            }
        });
        tasks.push(task)
    }

    // Spawn UI task in background
    let running_for_ui = running.clone();
    let terminal_guard_for_ui = terminal_guard.clone();
    let view_slot_for_ui = view_slot.clone();
    let theme_slot_for_ui = theme_slot.clone();
    let ip_data_for_ui = ip_data.clone();
    let errs_for_ui = errs.clone();

    let ui_task = task::spawn_blocking(move || {
        let mut guard = terminal_guard_for_ui.lock().unwrap();
        let _ = draw::draw_interface_with_updates(
            &mut guard.terminal.as_mut().unwrap(),
            view_slot_for_ui,
            theme_slot_for_ui,
            &ip_data_for_ui,
            ui_data_rx,
            running_for_ui,
            errs_for_ui,
            output_file,
        );
    });

    // Wait for all ping tasks to complete
    for task in tasks {
        task.await?;
    }
    
    // All ping tasks completed, signal UI to exit
    *running.lock().unwrap() = false;
    
    // Wait for UI task to finish
    ui_task.await?;
    
    // restore terminal
    draw::restore_terminal(&mut terminal_guard.lock().unwrap().terminal.as_mut().unwrap())?;

    Ok(())
}

async fn run_exporter_mode(
    targets: Vec<String>,
    interval: i32,
    port: u16,
    force_ipv6: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // 创建 Prometheus metrics 收集器
    let prometheus_metrics = Arc::new(PrometheusMetrics::new()?);

    // 创建信号处理通道
    let running = Arc::new(AtomicBool::new(true));
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));

    // 设置信号处理
    let running_for_signal = running.clone();
    let shutdown_tx_for_signal = shutdown_tx.clone();
    tokio::spawn(async move {
        match signal::ctrl_c().await {
            Ok(()) => {
                println!("\nReceived Ctrl+C, shutting down gracefully...");
                running_for_signal.store(false, Ordering::Relaxed);
                
                // 发送关闭信号给 HTTP 服务器
                if let Some(tx) = shutdown_tx_for_signal.lock().unwrap().take() {
                    let _ = tx.send(());
                }
            }
            Err(err) => {
                eprintln!("Unable to listen for shutdown signal: {}", err);
            }
        }
    });

    // Targets are already de-duplicated and non-empty by the time they reach
    // here (see config::resolve_exporter), but guard defensively.
    if targets.is_empty() {
        return Err("No valid targets provided".into());
    }

    // 解析目标地址为 IP 地址
    let mut target_pairs = Vec::new();
    for target in &targets {
        let ip = network::get_host_ipaddr(target, force_ipv6)?;
        target_pairs.push((target.clone(), ip));
    }

    println!("🚀 NBPing Prometheus Exporter Mode Started");
    println!("┌─────────────────────────────────────────────────────────");
    println!("│ Targets     : {} host(s)", targets.len());
    for (i, target) in targets.iter().enumerate() {
        if i < 5 {
            println!("│             : {}", target);
        } else if i == 5 {
            println!("│             : ... ({} more)", targets.len() - 5);
            break;
        }
    }
    println!("│ Interval    : {} seconds", interval);
    println!("│ Metrics port: {}", port);
    println!("│ Metrics     : http://0.0.0.0:{}/metrics", port);
    println!("│ Actions     : Press Ctrl+C or q to stop");
    println!("└─────────────────────────────────────────────────────────");

    // 启动 HTTP metrics 服务器
    let metrics_addr = format!("0.0.0.0:{}", port).parse()?;
    let metrics_for_server = prometheus_metrics.clone();
    let metrics_task = task::spawn(async move {
        http_server::start_metrics_server(
            metrics_for_server,
            metrics_addr,
            shutdown_rx,
        ).await
    });

    let interval_ms = interval.saturating_mul(1000);
    let ping_threads = spawn_ping_workers(
        target_pairs,
        Duration::from_millis(interval_ms as u64),
        running.clone(),
        prometheus_metrics.clone(),
    );

    // Listen for q/esc to exit (exporter mode only)
    let running_for_key = running.clone();
    let shutdown_tx_for_key = shutdown_tx.clone();
    let key_listener = std::thread::spawn(move || {
        let _raw_mode = match RawModeGuard::new() {
            Ok(guard) => guard,
            Err(_) => return,
        };

        while running_for_key.load(Ordering::Relaxed) {
            if let Ok(true) = event::poll(Duration::from_millis(50)) {
                if let Ok(Event::Key(key)) = event::read() {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            running_for_key.store(false, Ordering::Relaxed);
                            if let Some(tx) = shutdown_tx_for_key.lock().unwrap().take() {
                                let _ = tx.send(());
                            }
                            break;
                        }
                        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                            running_for_key.store(false, Ordering::Relaxed);
                            if let Some(tx) = shutdown_tx_for_key.lock().unwrap().take() {
                                let _ = tx.send(());
                            }
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    // Wait for metrics server to shut down
    let metrics_result = metrics_task.await?;
    let metrics_error = metrics_result.err();

    running.store(false, Ordering::Relaxed);

    // Wait for ping threads to complete
    for handle in ping_threads {
        let _ = handle.join();
    }

    let _ = key_listener.join();

    if let Some(err) = metrics_error {
        return Err(err);
    }

    Ok(())
}
