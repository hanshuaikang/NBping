mod network;
mod draw;
mod terminal;
mod ip_data;
mod ui;
mod ping_event;
mod data_processor;
mod exporter;

use clap::{Parser, Subcommand};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
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
    version = "v0.6.0",
    author = "hanshuaikang<https://github.com/hanshuaikang>",
    about = "🏎  Nping mean NB Ping, A Ping Tool in Rust with Real-Time Data and Visualizations"
)]
struct Args {
    /// Target IP address or hostname to ping
    #[arg(help = "target IP address or hostname to ping", required = false)]
    target: Vec<String>,

    /// Number of pings to send, when count is 0, the maximum number of pings per address is calculated
    #[arg(short, long, default_value_t = 0, help = "Number of pings to send")]
    count: usize,

    /// Interval in seconds between pings
    #[arg(short, long, default_value_t = 0, help = "Interval in seconds between pings")]
    interval: i32,

    #[clap(long = "force_ipv6", default_value_t = false, short = '6', help = "Force using IPv6")]
    pub force_ipv6: bool,

    #[arg(
        short = 'm',
        long,
        default_value_t = 0,
        help = "Specify the maximum number of target addresses, Only works on one target address"
    )]
    multiple: i32,

    #[arg(short, long, default_value = "graph", help = "View mode graph/table/point/sparkline")]
    view_type: String,

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
        #[arg(help = "target IP addresses or hostnames to ping", required = true)]
        target: Vec<String>,

        /// Interval in seconds between pings
        #[arg(short, long, default_value_t = 1, help = "Interval in seconds between pings")]
        interval: i32,

        /// Prometheus metrics HTTP port
        #[arg(short, long, default_value_t = 9090, help = "Prometheus metrics HTTP port")]
        port: u16,
    },
}


fn main() -> Result<(), Box<dyn std::error::Error>> {
    // parse command line arguments
    let args = Args::parse();

    match args.command {
        Some(Commands::Exporter { target, interval, port }) => {
            let worker_threads = (target.len() + 1).max(1);
            // Create tokio runtime for Exporter mode
            let rt = Builder::new_multi_thread()
                .worker_threads(worker_threads)
                .enable_all()
                .build()?;

            let res = rt.block_on(run_exporter_mode(target, interval, port));

            // if error print error message and exit
            if let Err(err) = res {
                eprintln!("{}", err);
                std::process::exit(1);
            }
        },
        None => {
            // Default ping mode
            if args.target.is_empty() {
                eprintln!("Error: target IP address or hostname is required");
                std::process::exit(1);
            }

            // set Ctrl+C and q and esc to exit
            let running = Arc::new(Mutex::new(true));

            // check output file
            if let Some(ref output_path) = args.output {
                if std::path::Path::new(output_path).exists() {
                    eprintln!("Output file already exists: {}", output_path);
                    std::process::exit(1);
                }
            }

            // after de-duplication, the original order is still preserved
            let mut seen = HashSet::new();
            let targets: Vec<String> = args.target.into_iter()
                .filter(|item| seen.insert(item.clone()))
                .collect();

            // Calculate worker threads based on IP count
            let ip_count = if targets.len() == 1 && args.multiple > 0 {
                args.multiple as usize
            } else {
                targets.len()
            };
            let worker_threads = (ip_count +  1).max(1);

            // Create tokio runtime with specific worker thread count
            let rt = Builder::new_multi_thread()
                .worker_threads(worker_threads)
                .enable_all()
                .build()?;

            let res = rt.block_on(run_app(targets, args.count, args.interval, running.clone(), args.force_ipv6, args.multiple, args.view_type, args.output));

            // if error print error message and exit
            if let Err(err) = res {
                eprintln!("{}", err);
                std::process::exit(1);
            }
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
    draw::init_terminal()?;

    // Create terminal instance
    let terminal = draw::init_terminal().unwrap();
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
        view_type.clone(),
        running.clone(),
    );

    let view_type = Arc::new(view_type);

    let errs = Arc::new(Mutex::new(Vec::new()));

    let interval = if interval == 0 { 500 } else { interval * 1000 };
    let mut tasks = Vec::new();


    // first draw ui
    {
        let mut guard = terminal_guard.lock().unwrap();
        let ip_data = ip_data.lock().unwrap();

        draw::draw_interface(
            &mut guard.terminal.as_mut().unwrap(),
            &view_type,
            &ip_data,
            &mut errs.lock().unwrap(),
        ).ok();
    }
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
    let view_type_for_ui = view_type.clone();
    let ip_data_for_ui = ip_data.clone();
    let errs_for_ui = errs.clone();
    
    let ui_task = task::spawn(async move {
        let mut guard = terminal_guard_for_ui.lock().unwrap();
        draw::draw_interface_with_updates(
            &mut guard.terminal.as_mut().unwrap(),
            &view_type_for_ui,
            &ip_data_for_ui,
            ui_data_rx,
            running_for_ui,
            errs_for_ui,
            output_file,
        ).ok();
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

    // 去重目标地址，同时保留原始顺序
    let mut seen = std::collections::HashSet::new();
    let targets: Vec<String> = targets.into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect();

    if targets.is_empty() {
        return Err("No valid targets provided".into());
    }

    // 解析目标地址为 IP 地址
    let mut target_pairs = Vec::new();
    for target in &targets {
        let ip = network::get_host_ipaddr(target, false)?;
        target_pairs.push((target.clone(), ip));
    }

    println!("🚀 NPing Prometheus Exporter Mode Started");
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

    let interval_ms = interval * 1000;
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
