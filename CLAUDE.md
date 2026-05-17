# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

NBping (formerly Nping) is a Rust CLI ping tool that wraps the system `ping` binary (via the `pinger` crate) and renders results in a Ratatui-based terminal UI, or exports them as Prometheus metrics. Binary name: `nbping`. The crate name in `Cargo.toml` is also `nbping`.

## Commands

```bash
cargo build              # debug build
cargo build --release    # release build (produces ./target/release/nbping)
cargo test               # run all tests (CI runs `cargo test --verbose`)
cargo test <name>        # run a single test by name substring
cargo run -- <targets>   # run default TUI ping mode
cargo run -- exporter <targets> -p 9100   # run Prometheus exporter mode
```

CI (`.github/workflows/rust.yml`) runs only `cargo build` + `cargo test` on Ubuntu. Releases are tag-triggered (`v*.*.*`) and cross-compile binaries for Linux/macOS/Windows via `taiki-e/upload-rust-binary-action` — keep the `bin: nbping` name in `release.yml` in sync with `Cargo.toml`.

## Architecture

There are **two execution modes**, selected by the presence of the `exporter` subcommand in `main.rs`. Both modes build their own multi-threaded Tokio runtime in `main()` sized to `target_count + 1` worker threads — this is intentional so each ping target gets a dedicated worker without contention.

### Default TUI mode (`run_app` in `main.rs`)

Data flows through two sync `mpsc` channels (capacity 0, so producers block until consumed — this provides backpressure):

```
network::send_ping (per-target tokio task)
    └── PingEvent ──> data_processor (std::thread)
                          └── IpData ──> draw::draw_interface_with_updates (tokio task)
                                              └── ratatui frame
```

- `network.rs` — `PingTask::run` spawns one task per resolved IP. It reads from the `pinger` crate's blocking `stream.recv()` and emits `PingEvent::Success { rtt }` or `PingEvent::Timeout`. Hard errors are pushed to a shared `errs: Arc<Mutex<Vec<String>>>` instead of crashing.
- `data_processor.rs` — single background thread that owns a `HashMap<addr_ip, IpData>` and rolls up min/max/last RTT, timeout counter, and a sliding `VecDeque<f64>` of recent RTTs. The deque length is a single shared `RTT_RETENTION = 200` (constant in `data_processor.rs`) because views can now be switched at runtime (`1`–`4` / `Tab`), so retention can't depend on the initial view. Renderers windowing-trim as needed (e.g. `graph.rs` keeps a `GRAPH_WINDOW = 60` slice). Timeouts are encoded as `-1.0` in the RTT buffer; renderers must handle this sentinel.
- `draw.rs` — owns the ratatui terminal, the keyboard event loop (q/Esc/Ctrl+C set `running = false`), and optional `--output` file writing. It dispatches to one of four view renderers in `src/ui/` based on the `view_type` string.
- `src/ui/{graph,table,point,sparkline}.rs` — each is an independent renderer; `mod.rs` re-exports the four `draw_*_view` functions. When adding a new view: add a variant to `View` in `view.rs` (and update `from_str`/`from_u8`/`name`/`next`), register it in `ui/mod.rs`, add a match arm in `draw::draw_interface`, and add a `KeyCode::Char('N')` arm in `draw_interface_with_updates`'s event loop.
- `terminal.rs` — `TerminalGuard` ensures `restore_terminal` (disable raw mode, leave alt screen) runs on drop. The `RawModeGuard` in `main.rs` plays the same role for exporter mode's key listener.

The `running: Arc<Mutex<bool>>` flag is the universal shutdown signal across all default-mode threads/tasks.

### Exporter mode (`run_exporter_mode` in `main.rs`, `src/exporter/`)

Different concurrency model — uses `std::thread` (not tokio tasks) for ping workers because the metrics path doesn't need async, and `AtomicBool` (not `Mutex<bool>`) for the running flag.

- `exporter/runner.rs::spawn_ping_workers` — one OS thread per target, each running `run_ping_loop` that updates `PrometheusMetrics` directly (no channels).
- `exporter/metric.rs` — defines the Prometheus collectors and contains the `http_server` submodule (hyper 1.x + hyper-util) that serves `/metrics`. Shutdown is signaled via a `tokio::sync::oneshot` channel from either the Ctrl+C handler or the q/Esc key listener.

When changing metric names or labels in `metric.rs`, remember they're a public scrape contract — existing Grafana dashboards and Prometheus queries depend on them.

### Cross-cutting

- `ip_data.rs` — the `IpData` struct is the shared data shape between the processor and all UI renderers. Adding a field requires updating both the initial construction in `main.rs` / `data_processor.rs` and any renderer that reads it.
- `network::resolve_host_ips` is used by both modes. The `--multiple` / `-m` flag (default mode only) returns multiple A/AAAA records for a single host so you can ping all of them in parallel — this is why default mode's IP count can exceed target count.
- `force_ipv6` filters resolved addresses to V6 only; default behavior filters to V4 only (there's no dual-stack mode).
