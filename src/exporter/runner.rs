use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::Duration;

use pinger::{ping, PingOptions, PingResult};

use crate::exporter::PrometheusMetrics;

pub fn spawn_ping_workers(
    targets: Vec<(String, String)>,
    interval: Duration,
    running: Arc<AtomicBool>,
    metrics: Arc<PrometheusMetrics>,
) -> Vec<thread::JoinHandle<()>> {
    targets
        .into_iter()
        .map(|(addr, ip)| {
            let running = running.clone();
            let metrics = metrics.clone();
            let interval = interval;
            thread::spawn(move || run_ping_loop(addr, ip, interval, running, metrics))
        })
        .collect()
}

fn run_ping_loop(
    addr: String,
    ip: String,
    interval: Duration,
    running: Arc<AtomicBool>,
    metrics: Arc<PrometheusMetrics>,
) {
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
