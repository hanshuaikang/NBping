mod network;
mod draw;
mod terminal;
mod ip_data;
mod ui;
mod ping_event;
mod data_processor;
mod metric;

use clap::{Parser, Subcommand};
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::{task, runtime::Builder};
use crate::ip_data::IpData;
use crate::ping_event::PingEvent;
use crate::data_processor::start_data_processor;
use std::sync::mpsc;
use crate::network::send_ping;

#[derive(Parser, Debug)]
#[command(
    version = "v0.5.0",
    author = "hanshuaikang<https://github.com/hanshuaikang>",
    about = "🏎  Nping mean NB Ping, A Ping Tool in Rust with Real-Time Data and Visualizations"
)]
struct Args {
    /// Target IP address or hostname to ping
    #[arg(help = "target IP address or hostname to ping")]
    target: Vec<String>,

    /// Number of pings to send, when count is 0, the maximum number of pings per address is calculated
    #[arg(short, long, default_value_t = 65535, help = "Number of pings to send")]
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
    /// Agent mode for monitoring
    Agent {
        /// Target IP addresses or hostnames to ping
        #[arg(help = "target IP addresses or hostnames to ping", required = true)]
        target: Vec<String>,

        /// Interval in seconds between pings
        #[arg(short, long, default_value_t = 1, help = "Interval in seconds between pings")]
        interval: i32,

        /// Port to monitor (default: ICMP ping, no port specified)
        #[arg(short, long, help = "Port to monitor")]
        port: Option<u16>,
    },
}


fn main() -> Result<(), Box<dyn std::error::Error>> {
    // parse command line arguments
    let args = Args::parse();

    match args.command {
        Some(Commands::Agent { target, interval, port }) => {
            // Create tokio runtime for agent mode
            let rt = Builder::new_multi_thread()
                .worker_threads(2) // Simple setup for agent mode
                .enable_all()
                .build()?;

            let res = rt.block_on(run_agent_mode(target, interval, port));

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

async fn run_agent_mode(
    targets: Vec<String>,
    interval: i32,
    port: Option<u16>,
) -> Result<(), Box<dyn std::error::Error>> {
    // set Ctrl+C to exit
    let running = Arc::new(Mutex::new(true));

    // ping event channel (network -> data processor)
    let (ping_event_tx, ping_event_rx) = mpsc::sync_channel::<PingEvent>(0);
    
    // ui data channel (data processor -> ui) - not used in agent mode but required for start_data_processor
    let (ui_data_tx, _ui_data_rx) = mpsc::sync_channel::<IpData>(0);

    let ping_event_tx = Arc::new(ping_event_tx);

    // after de-duplication, the original order is still preserved
    let mut seen = std::collections::HashSet::new();
    let targets: Vec<String> = targets.into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect();

    if targets.is_empty() {
        return Err("No valid targets provided".into());
    }

    // Get IP addresses for all targets
    let mut ips = Vec::new();
    for target in &targets {
        let ip = network::get_host_ipaddr(target, false)?;
        ips.push(ip);
    }

    // Define initial data for all targets
    let ip_data = Arc::new(Mutex::new(ips.iter().enumerate().map(|(i, _)| IpData {
        ip: String::new(),
        addr: targets[i].clone(),
        rtts: VecDeque::new(),
        last_attr: 0.0,
        min_rtt: 0.0,
        max_rtt: 0.0,
        timeout: 0,
        received: 0,
        pop_count: 0,
    }).collect::<Vec<_>>()));

    // Start data processor - will handle metric recording logic
    let targets_for_processor: Vec<(String, String)> = ips.iter().enumerate().map(|(i, ip)| {
        (targets[i].clone(), ip.clone())
    }).collect();
    
    start_data_processor(
        ping_event_rx,
        ui_data_tx,
        targets_for_processor,
        "agent".to_string(), // Use "agent" as view_type to distinguish from normal ping mode
        running.clone(),
    );

    let errs = Arc::new(Mutex::new(Vec::new()));
    let interval_ms = interval * 1000;
    let mut tasks = Vec::new();

    // Start ping tasks for all targets
    for (i, ip) in ips.iter().enumerate() {
        let ip = ip.clone();
        let running_clone = running.clone();
        let errs_clone = errs.clone();
        let ping_event_tx_clone = ping_event_tx.clone();
        let ip_data_clone = ip_data.clone();
        
        let task = task::spawn({
            let mut data = ip_data_clone.lock().unwrap();
            // update the ip
            data[i].ip = ip.clone();
            let addr = data[i].addr.clone();
            drop(data);
            
            async move {
                send_ping(
                    addr, 
                    ip, 
                    errs_clone, 
                    0, // count=0 for infinite ping in agent mode
                    interval_ms, 
                    running_clone, 
                    ping_event_tx_clone
                ).await.unwrap();
            }
        });
        tasks.push(task);
    }

    println!("Agent mode started for {} target(s): {:?}", targets.len(), targets);
    if let Some(p) = port {
        println!("Monitoring port: {}", p);
    }
    println!("Ping interval: {} seconds", interval);
    println!("Press Ctrl+C to stop");

    // Wait for all ping tasks to complete
    for task in tasks {
        task.await?;
    }
    
    println!("Agent mode stopped");
    Ok(())
}