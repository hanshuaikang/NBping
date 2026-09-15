use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::Duration;

use pinger::{ping, PingOptions, PingResult};

use crate::exporter::PrometheusMetrics;
use crate::network::tcp_ping;

pub fn spawn_ping_workers(
    targets: Vec<(String, String)>,
    interval: Duration,
    running: Arc<AtomicBool>,
    metrics: Arc<PrometheusMetrics>,
    tcp_port: Option<u16>,
) -> Vec<thread::JoinHandle<()>> {
    targets
        .into_iter()
        .map(|(addr, ip)| {
            let running = running.clone();
            let metrics = metrics.clone();
            let interval = interval;
            thread::spawn(move || run_ping_loop(addr, ip, interval, running, metrics, tcp_port))
        })
        .collect()
}

fn run_ping_loop(
    addr: String,
    ip: String,
    interval: Duration,
    running: Arc<AtomicBool>,
    metrics: Arc<PrometheusMetrics>,
    tcp_port: Option<u16>,
) {
    if let Some(port) = tcp_port {
        run_tcp_ping_loop(addr, ip, port, interval, running, metrics);
        return;
    }

    let options = PingOptions::new(ip.clone(), interval, None);
    let stream = match ping(options) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("host({}) ping err, reason: ping init failed, err: {}", ip, err);
            return;
        }
    };

    while running.load(Ordering::Relaxed) {
        match stream.recv() {
            Ok(PingResult::Pong(duration, _size)) => {
                let rtt_ms = duration.as_secs_f64() * 1000.0;
                metrics.record_ping_success(&addr, &ip, rtt_ms);
            }
            Ok(PingResult::Timeout(_)) => {
                metrics.record_ping_timeout(&addr, &ip);
            }
            Ok(PingResult::PingExited(status, err)) => {
                if status.code() != Some(0) {
                    eprintln!(
                        "host({}) ping err, reason: ping exited, status: {} err: {}",
                        ip, err, status
                    );
                    metrics.record_ping_error(&addr, &ip);
                }
            }
            Ok(PingResult::Unknown(msg)) => {
                eprintln!("host({}) ping err, reason: unknown, err: {}", ip, msg);
                metrics.record_ping_error(&addr, &ip);
            }
            Err(err) => {
                eprintln!("host({}) ping err, reason: recv failed, err: {}", ip, err);
                metrics.record_ping_error(&addr, &ip);
            }
        }
    }
}

fn run_tcp_ping_loop(
    addr: String,
    ip: String,
    port: u16,
    interval: Duration,
    running: Arc<AtomicBool>,
    metrics: Arc<PrometheusMetrics>,
) {
    while running.load(Ordering::Relaxed) {
        let probe_started_at = std::time::Instant::now();
        match tcp_ping(&ip, port) {
            Ok(duration) => {
                metrics.record_ping_success(&addr, &ip, duration.as_secs_f64() * 1000.0);
            }
            Err(_) => {
                metrics.record_ping_timeout(&addr, &ip);
            }
        }

        let mut remaining = interval.saturating_sub(probe_started_at.elapsed());
        while remaining > Duration::ZERO && running.load(Ordering::Relaxed) {
            let sleep_for = remaining.min(Duration::from_millis(100));
            thread::sleep(sleep_for);
            remaining = remaining.saturating_sub(sleep_for);
        }
    }
}
