use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use rusqlite::{params, Connection, OpenFlags, Row};

use crate::ping_event::PingEvent;

const HISTORY_CHANNEL_CAPACITY: usize = 1024;
const HISTORY_BATCH_SIZE: usize = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeRecord {
    pub sampled_at_ms: i64,
    pub target: String,
    pub ip: String,
    pub probe: String,
    pub port: Option<u16>,
    pub status: String,
    pub rtt_ms: Option<f64>,
    pub error: Option<String>,
}

impl ProbeRecord {
    pub fn from_event(event: &PingEvent) -> Self {
        match event {
            PingEvent::Success {
                addr,
                ip,
                rtt,
                sampled_at_ms,
                tcp_port,
            } => Self {
                sampled_at_ms: *sampled_at_ms,
                target: addr.clone(),
                ip: ip.clone(),
                probe: probe_name(*tcp_port).to_string(),
                port: *tcp_port,
                status: "success".to_string(),
                rtt_ms: Some(*rtt),
                error: None,
            },
            PingEvent::Timeout {
                addr,
                ip,
                sampled_at_ms,
                tcp_port,
                error,
            } => Self {
                sampled_at_ms: *sampled_at_ms,
                target: addr.clone(),
                ip: ip.clone(),
                probe: probe_name(*tcp_port).to_string(),
                port: *tcp_port,
                status: "timeout".to_string(),
                rtt_ms: None,
                error: error.clone(),
            },
        }
    }
}

fn probe_name(tcp_port: Option<u16>) -> &'static str {
    if tcp_port.is_some() {
        "tcp"
    } else {
        "icmp"
    }
}

pub struct HistoryWriter {
    sender: Option<SyncSender<ProbeRecord>>,
    handle: Option<JoinHandle<Result<()>>>,
}

impl HistoryWriter {
    pub fn start(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let connection = Connection::open(path)
            .with_context(|| format!("failed to open history database: {}", path.display()))?;
        initialize_schema(&connection)?;

        let (sender, receiver) = mpsc::sync_channel(HISTORY_CHANNEL_CAPACITY);
        let handle = thread::spawn(move || write_records(connection, receiver));
        Ok(Self {
            sender: Some(sender),
            handle: Some(handle),
        })
    }

    pub fn sender(&self) -> SyncSender<ProbeRecord> {
        self.sender
            .as_ref()
            .expect("history writer is active")
            .clone()
    }

    pub fn finish(mut self) -> Result<()> {
        self.sender.take();
        self.join_writer()
    }

    fn join_writer(&mut self) -> Result<()> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        handle
            .join()
            .map_err(|_| anyhow!("history writer thread panicked"))?
    }
}

impl Drop for HistoryWriter {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn initialize_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS probe_history (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             sampled_at_ms INTEGER NOT NULL,
             target TEXT NOT NULL,
             ip TEXT NOT NULL,
             probe TEXT NOT NULL,
             port INTEGER,
             status TEXT NOT NULL,
             rtt_ms REAL,
             error TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_probe_history_time
             ON probe_history(sampled_at_ms, id);
         CREATE INDEX IF NOT EXISTS idx_probe_history_target_time
             ON probe_history(target, sampled_at_ms, id);",
    )?;
    Ok(())
}

fn write_records(mut connection: Connection, receiver: Receiver<ProbeRecord>) -> Result<()> {
    while let Ok(first) = receiver.recv() {
        let mut batch = Vec::with_capacity(HISTORY_BATCH_SIZE);
        batch.push(first);
        while batch.len() < HISTORY_BATCH_SIZE {
            match receiver.try_recv() {
                Ok(record) => batch.push(record),
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
            }
        }

        let transaction = connection.transaction()?;
        {
            let mut statement = transaction.prepare_cached(
                "INSERT INTO probe_history
                 (sampled_at_ms, target, ip, probe, port, status, rtt_ms, error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for record in batch {
                statement.execute(params![
                    record.sampled_at_ms,
                    record.target,
                    record.ip,
                    record.probe,
                    record.port,
                    record.status,
                    record.rtt_ms,
                    record.error,
                ])?;
            }
        }
        transaction.commit()?;
    }
    Ok(())
}

pub fn parse_query_time(value: &str) -> Result<i64> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.timestamp_millis());
    }

    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            let local = Local
                .from_local_datetime(&parsed)
                .single()
                .ok_or_else(|| anyhow!("local time is ambiguous or does not exist: {}", value))?;
            return Ok(local.timestamp_millis());
        }
    }

    Err(anyhow!(
        "invalid time '{}'; use RFC3339 or YYYY-MM-DD HH:MM[:SS]",
        value
    ))
}

pub fn query_records(
    path: impl AsRef<Path>,
    from_ms: i64,
    to_ms: i64,
    target: Option<&str>,
) -> Result<Vec<ProbeRecord>> {
    if from_ms > to_ms {
        return Err(anyhow!("history range start must not be after end"));
    }

    let path = path.as_ref();
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open history database: {}", path.display()))?;

    let rows = if let Some(target) = target {
        query_with_params(
            &connection,
            "SELECT sampled_at_ms, target, ip, probe, port, status, rtt_ms, error
             FROM probe_history
             WHERE sampled_at_ms >= ?1 AND sampled_at_ms <= ?2 AND target = ?3
             ORDER BY sampled_at_ms ASC, id ASC",
            params![from_ms, to_ms, target],
        )?
    } else {
        query_with_params(
            &connection,
            "SELECT sampled_at_ms, target, ip, probe, port, status, rtt_ms, error
             FROM probe_history
             WHERE sampled_at_ms >= ?1 AND sampled_at_ms <= ?2
             ORDER BY sampled_at_ms ASC, id ASC",
            params![from_ms, to_ms],
        )?
    };

    Ok(rows)
}

fn query_with_params<P>(
    connection: &Connection,
    sql: &str,
    parameters: P,
) -> Result<Vec<ProbeRecord>>
where
    P: rusqlite::Params,
{
    let mut statement = connection.prepare(sql)?;
    let records = statement
        .query_map(parameters, map_record)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(records)
}

fn map_record(row: &Row<'_>) -> rusqlite::Result<ProbeRecord> {
    Ok(ProbeRecord {
        sampled_at_ms: row.get(0)?,
        target: row.get(1)?,
        ip: row.get(2)?,
        probe: row.get(3)?,
        port: row.get(4)?,
        status: row.get(5)?,
        rtt_ms: row.get(6)?,
        error: row.get(7)?,
    })
}

pub fn print_history(
    path: impl AsRef<Path>,
    from: &str,
    to: &str,
    target: Option<&str>,
) -> Result<()> {
    let from_ms = parse_query_time(from)?;
    let to_ms = parse_query_time(to)?;
    let records = query_records(path.as_ref(), from_ms, to_ms, target)?;

    println!("History database: {}", path.as_ref().display());
    println!(
        "Range: {} -> {}",
        format_timestamp(from_ms)?,
        format_timestamp(to_ms)?
    );
    if let Some(target) = target {
        println!("Target: {}", target);
    }

    let success = records
        .iter()
        .filter(|record| record.status == "success")
        .count();
    let timeout = records.len().saturating_sub(success);
    let availability = if records.is_empty() {
        0.0
    } else {
        success as f64 * 100.0 / records.len() as f64
    };
    let rtts: Vec<f64> = records.iter().filter_map(|record| record.rtt_ms).collect();

    println!(
        "Summary: total={} success={} timeout={} availability={:.2}%{}",
        records.len(),
        success,
        timeout,
        availability,
        format_rtt_summary(&rtts)
    );

    if records.is_empty() {
        println!("No probe records found in this range.");
        return Ok(());
    }

    println!("TIME\tTARGET\tIP\tPROBE\tSTATUS\tRTT_MS\tERROR");
    for record in records {
        let port = record
            .port
            .map(|value| format!(":{}", value))
            .unwrap_or_default();
        let rtt = record
            .rtt_ms
            .map(|value| format!("{:.2}", value))
            .unwrap_or_else(|| "-".to_string());
        let error = record
            .error
            .unwrap_or_default()
            .replace(['\r', '\n', '\t'], " ");
        println!(
            "{}\t{}\t{}\t{}{}\t{}\t{}\t{}",
            format_timestamp(record.sampled_at_ms)?,
            record.target,
            record.ip,
            record.probe,
            port,
            record.status,
            rtt,
            error
        );
    }

    Ok(())
}

fn format_timestamp(timestamp_ms: i64) -> Result<String> {
    let utc = Utc
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .ok_or_else(|| anyhow!("timestamp is out of range: {}", timestamp_ms))?;
    Ok(utc
        .with_timezone(&Local)
        .to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn format_rtt_summary(rtts: &[f64]) -> String {
    if rtts.is_empty() {
        return String::new();
    }
    let min = rtts.iter().copied().fold(f64::INFINITY, f64::min);
    let max = rtts.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let average = rtts.iter().sum::<f64>() / rtts.len() as f64;
    format!(" avg={:.2}ms min={:.2}ms max={:.2}ms", average, min, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDatabase {
        path: PathBuf,
    }

    impl TestDatabase {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self {
                path: std::env::temp_dir().join(format!(
                    "nbping-history-{}-{}.db",
                    std::process::id(),
                    unique
                )),
            }
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(format!("{}-wal", self.path.display()));
            let _ = std::fs::remove_file(format!("{}-shm", self.path.display()));
        }
    }

    fn record(sampled_at_ms: i64, target: &str, status: &str, rtt_ms: Option<f64>) -> ProbeRecord {
        ProbeRecord {
            sampled_at_ms,
            target: target.to_string(),
            ip: "127.0.0.1".to_string(),
            probe: "tcp".to_string(),
            port: Some(443),
            status: status.to_string(),
            rtt_ms,
            error: (status == "timeout").then(|| "timed out".to_string()),
        }
    }

    #[test]
    fn writer_appends_and_drains_all_records() {
        let database = TestDatabase::new();
        for range in [0..50, 50..100] {
            let writer = HistoryWriter::start(&database.path).unwrap();
            let sender = writer.sender();
            for timestamp in range {
                sender
                    .send(record(timestamp, "example.com", "success", Some(1.5)))
                    .unwrap();
            }
            drop(sender);
            writer.finish().unwrap();
        }

        let records = query_records(&database.path, 0, 99, None).unwrap();
        assert_eq!(records.len(), 100);
        assert_eq!(records.first().unwrap().sampled_at_ms, 0);
        assert_eq!(records.last().unwrap().sampled_at_ms, 99);
    }

    #[test]
    fn query_uses_inclusive_range_order_and_target_filter() {
        let database = TestDatabase::new();
        let writer = HistoryWriter::start(&database.path).unwrap();
        let sender = writer.sender();
        for item in [
            record(30, "other.example", "success", Some(3.0)),
            record(20, "example.com", "timeout", None),
            record(10, "example.com", "success", Some(1.0)),
            record(40, "example.com", "success", Some(4.0)),
        ] {
            sender.send(item).unwrap();
        }
        drop(sender);
        writer.finish().unwrap();

        let records = query_records(&database.path, 10, 20, Some("example.com")).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.sampled_at_ms)
                .collect::<Vec<_>>(),
            vec![10, 20]
        );
        assert_eq!(records[1].status, "timeout");
        assert_eq!(records[1].rtt_ms, None);
    }

    #[test]
    fn query_rejects_reversed_range() {
        let database = TestDatabase::new();
        let error = query_records(&database.path, 20, 10, None).unwrap_err();
        assert!(error.to_string().contains("start must not be after end"));
    }

    #[test]
    fn parses_rfc3339_and_local_time() {
        assert_eq!(
            parse_query_time("2026-09-15T06:00:00Z").unwrap(),
            1_789_452_000_000
        );
        assert!(parse_query_time("2026-09-15 14:00:00").is_ok());
        assert!(parse_query_time("not-a-time").is_err());
    }

    #[test]
    fn converts_events_without_changing_timeout_semantics() {
        let event = PingEvent::Timeout {
            addr: "example.com".to_string(),
            ip: "127.0.0.1".to_string(),
            sampled_at_ms: 42,
            tcp_port: Some(443),
            error: Some("connection refused".to_string()),
        };
        let record = ProbeRecord::from_event(&event);
        assert_eq!(record.probe, "tcp");
        assert_eq!(record.status, "timeout");
        assert_eq!(record.error.as_deref(), Some("connection refused"));
    }

    #[test]
    fn rtt_summary_handles_successes_and_all_timeouts() {
        assert_eq!(format_rtt_summary(&[]), "");
        assert_eq!(
            format_rtt_summary(&[10.0, 20.0, 30.0]),
            " avg=20.00ms min=10.00ms max=30.00ms"
        );
    }
}
