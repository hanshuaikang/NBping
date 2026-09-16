use std::error::Error;
use std::io;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};
use anyhow::{anyhow, Context};
use chrono::Utc;

use pinger::{ping, PingOptions, PingResult};
use crate::ping_event::PingEvent;

const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

// get host ip address default to ipv4
pub(crate) fn resolve_host_ips(host: &str, force_ipv6: bool) -> Result<Vec<IpAddr>, Box<dyn Error>> {

    // get ip address
    let ipaddr: Vec<_> = (host, 80)
        .to_socket_addrs()
        .with_context(|| format!("failed to resolve host: {}", host))?
        .map(|s| s.ip())
        .collect();

    if ipaddr.is_empty() {
        return Err(anyhow!("Could not resolve host: {}", host).into());
    }

    // filter ipv4 or ipv6
    let filtered_ips: Vec<IpAddr> = if force_ipv6 {
        ipaddr.into_iter()
            .filter(|ip| matches!(ip, IpAddr::V6(_)))
            .collect()
    } else {
        ipaddr.into_iter()
            .filter(|ip| matches!(ip, IpAddr::V4(_)))
            .collect()
    };

    if filtered_ips.is_empty() {
        return Err(anyhow!("Could not resolve host: {}", host).into());
    }

    Ok(filtered_ips)
}

pub(crate) fn get_host_ipaddr(host: &str, force_ipv6: bool) -> Result<String, Box<dyn Error>> {
    let ips = resolve_host_ips(host, force_ipv6)?;
    Ok(ips[0].to_string())
}

pub(crate) fn get_multiple_host_ipaddr(host: &str, force_ipv6: bool, multiple: usize) -> Result<Vec<String>, Box<dyn Error>> {
    let ips = resolve_host_ips(host, force_ipv6)?;
    Ok(ips.into_iter()
        .take(multiple)
        .map(|ip| ip.to_string())
        .collect())
}

pub(crate) fn tcp_ping(ip: &str, port: u16) -> io::Result<Duration> {
    let ip_addr = ip.parse::<IpAddr>().map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid IP address '{}': {}", ip, err),
        )
    })?;
    let socket_addr = SocketAddr::new(ip_addr, port);
    let started_at = Instant::now();
    TcpStream::connect_timeout(&socket_addr, TCP_CONNECT_TIMEOUT)?;
    Ok(started_at.elapsed())
}


pub struct PingTask {
    addr: String,
    ip: String,
    count: usize,
    interval: u64,
    running: Arc<Mutex<bool>>,
    errs: Arc<Mutex<Vec<String>>>,
    tcp_port: Option<u16>,
}

impl PingTask {
    pub fn new(
        addr: String,
        ip: String,
        count: usize,
        interval: u64,
        running: Arc<Mutex<bool>>,
        errs: Arc<Mutex<Vec<String>>>,
        tcp_port: Option<u16>,
    ) -> Self {
        Self {
            addr,
            ip,
            count,
            interval,
            running,
            errs,
            tcp_port,
        }
    }

    pub async fn run(&self, ping_event_tx: Arc<SyncSender<PingEvent>>) -> Result<(), Box<dyn Error>>
    {
        if let Some(port) = self.tcp_port {
            return self.run_tcp(port, ping_event_tx).await;
        }

        // interval defined 0.5s/every ping
        let interval = Duration::from_millis(self.interval);
        let options = PingOptions::new(
            self.ip.clone(),
            interval,
            None,
        );

        // star ping
        let stream = ping(options)?;

        let mut ping_count = 0;
        loop {
            // if ctrl+c is pressed, break the loop
            if !*self.running.lock().unwrap() {
                break;
            }
            
            // if count is not 0, check if we've reached the limit
            if self.count > 0 {
                if ping_count >= self.count {
                    break;
                }
                ping_count += 1;
            }

            match stream.recv() {
                Ok(result) => {
                    match result {
                        PingResult::Pong(duration, _size) => {
                            // calculate rtt
                            let rtt = duration.as_secs_f64() * 1000.0;
                            let rtt_display: f64 = format!("{:.2}", rtt).parse().unwrap();
                            
                            let event = PingEvent::Success {
                                addr: self.addr.clone(),
                                ip: self.ip.clone(),
                                rtt: rtt_display,
                                sampled_at_ms: Utc::now().timestamp_millis(),
                                tcp_port: None,
                            };
                            
                            if ping_event_tx.send(event).is_err() {
                                break;
                            }
                        }
                        PingResult::Timeout(_) => {
                            let event = PingEvent::Timeout {
                                addr: self.addr.clone(),
                                ip: self.ip.clone(),
                                sampled_at_ms: Utc::now().timestamp_millis(),
                                tcp_port: None,
                                error: None,
                            };
                            
                            if ping_event_tx.send(event).is_err() {
                                break;
                            }
                        }
                        PingResult::PingExited(status, err) => {
                            if status.code() != Option::from(0) {
                                let err = format!("host({}) ping err, reason: ping excited, status: {} err: {}", self.ip, err, status);
                                set_error(self.errs.clone(), err);
                            }
                        }
                        PingResult::Unknown(msg) => {
                            let err = format!("host({}) ping err, reason:unknown, err: {}", self.ip, msg);
                            set_error(self.errs.clone(), err);
                        }
                    }
                }
                Err(err) => {
                    let err = format!("host({}) ping err, reason: unknown, err: {}", self.ip, err);
                    set_error(self.errs.clone(), err);
                }
            }


        }

        Ok(())
    }

    async fn run_tcp(
        &self,
        port: u16,
        ping_event_tx: Arc<SyncSender<PingEvent>>,
    ) -> Result<(), Box<dyn Error>> {
        let interval = Duration::from_millis(self.interval);
        let mut ping_count = 0;

        loop {
            if !*self.running.lock().unwrap() {
                break;
            }

            if self.count > 0 {
                if ping_count >= self.count {
                    break;
                }
                ping_count += 1;
            }

            let probe_started_at = Instant::now();
            let event = match tcp_ping(&self.ip, port) {
                Ok(duration) => PingEvent::Success {
                    addr: self.addr.clone(),
                    ip: self.ip.clone(),
                    rtt: (duration.as_secs_f64() * 100_000.0).round() / 100.0,
                    sampled_at_ms: Utc::now().timestamp_millis(),
                    tcp_port: Some(port),
                },
                Err(error) => PingEvent::Timeout {
                    addr: self.addr.clone(),
                    ip: self.ip.clone(),
                    sampled_at_ms: Utc::now().timestamp_millis(),
                    tcp_port: Some(port),
                    error: Some(error.to_string()),
                },
            };

            if ping_event_tx.send(event).is_err() {
                break;
            }

            if self.count > 0 && ping_count >= self.count {
                break;
            }

            let mut remaining = interval.saturating_sub(probe_started_at.elapsed());
            while remaining > Duration::ZERO && *self.running.lock().unwrap() {
                let sleep_for = remaining.min(Duration::from_millis(100));
                tokio::time::sleep(sleep_for).await;
                remaining = remaining.saturating_sub(sleep_for);
            }
        }

        Ok(())
    }
}

// send ping to the target address
pub async fn send_ping(
    addr: String,
    ip: String,
    errs: Arc<Mutex<Vec<String>>>,
    count: usize,
    interval: i32,
    running: Arc<Mutex<bool>>,
    ping_event_tx: Arc<SyncSender<PingEvent>>,
    tcp_port: Option<u16>,
) -> Result<(), Box<dyn Error>>
{
    // draw ui first
    let task = PingTask::new(
        addr.to_string(),
        ip,
        count,
        interval as u64,
        running,
        errs,
        tcp_port,
    );
    Ok(task.run(ping_event_tx).await?)
}


fn set_error(errs: Arc<Mutex<Vec<String>>>, err: String) {
    let mut err_list = errs.lock().unwrap();
    err_list.push(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener};

    #[test]
    fn tcp_ping_measures_a_successful_connection() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let accept_thread = std::thread::spawn(move || listener.accept().unwrap());

        let duration = tcp_ping("127.0.0.1", port).unwrap();

        assert!(duration < TCP_CONNECT_TIMEOUT);
        accept_thread.join().unwrap();
    }

    #[test]
    fn tcp_ping_rejects_an_invalid_ip() {
        let err = tcp_ping("not-an-ip", 80).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
