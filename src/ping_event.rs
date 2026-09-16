#[derive(Debug, Clone)]
pub enum PingEvent {
    Success {
        addr: String,
        ip: String,
        rtt: f64,
        sampled_at_ms: i64,
        tcp_port: Option<u16>,
    },
    Timeout {
        addr: String,
        ip: String,
        sampled_at_ms: i64,
        tcp_port: Option<u16>,
        error: Option<String>,
    },
}
