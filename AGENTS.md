# AGENTS.md

Project-specific instructions for working on NBping with Codex or another coding agent.

## Project

NBping is a Rust 2021 CLI Ping tool. It wraps the system `ping` command through the
`pinger` crate and supports two modes:

- TUI mode: concurrent Ping with Ratatui views and optional file output.
- Exporter mode: concurrent Ping with Prometheus metrics served over HTTP.

The binary and crate name are both `nbping`. The current package version is `0.7.1`.

## Commands

```text
cargo build
cargo build --release
cargo test
cargo test <name>
cargo run -- <targets>
cargo run -- <targets> --tcp-port 443
cargo run -- exporter <targets> -i 1 -p 9100
cargo run -- exporter <targets> --tcp-port 443 -i 1 -p 9100
```

CI runs `cargo build` and `cargo test` on Ubuntu. Release builds are produced for
Linux, macOS, and Windows; keep the binary name `nbping` aligned with `Cargo.toml`
and `.github/workflows/release.yml`.

## Repository Navigation

This repository contains a `.codegraph/` index. When exploring existing code, use
`codegraph explore "<question or symbol>"` before broad text searches. Re-read the
affected files after any change that may have been made by another process.

## Configuration

Configuration is loaded in `src/config.rs` and resolved in this order:

```text
explicit CLI value > YAML config value > built-in default
```

The `exporter` subcommand always selects exporter mode. Without that subcommand,
the YAML `mode` field selects `tui` or `exporter`, defaulting to `tui`.

Targets fall back from CLI to YAML and are de-duplicated while preserving order.
The default TUI interval is `0`, which means 500 ms at runtime. Exporter intervals
must be positive and fall back to one second when configured as zero or negative.
IPv4 is selected by default; `force_ipv6` selects IPv6 only.
ICMP is the default probe. `tcp: true` or a configured `tcp_port` selects TCP
connect probes; an explicit YAML `tcp: false` keeps ICMP enabled. The default TCP
destination port is 80. The CLI equivalents are `--tcp` and `--tcp-port`, and a
CLI `--tcp-port` always enables TCP mode.

## TUI Architecture

The default path is orchestrated by `src/main.rs`:

```text
network::send_ping (one Tokio task per resolved IP)
    -> PingEvent through a synchronous mpsc channel
    -> data_processor background thread
    -> IpData through a synchronous mpsc channel
    -> draw::draw_interface_with_updates
    -> Ratatui renderer
```

- `src/network.rs` resolves hosts, consumes the blocking ICMP `pinger` stream,
  and measures TCP connection establishment when TCP mode is selected.
- `src/data_processor.rs` owns a `HashMap<addr_ip, IpData>` and updates RTT,
  min/max, received count, timeout count, and the retained RTT window.
- Timeout samples are represented as `-1.0`; renderers must filter or display
  this sentinel intentionally.
- `RTT_RETENTION` is currently 200. Renderers may use smaller display windows.
- `src/draw.rs` owns terminal setup, keyboard handling, rendering, and optional
  output-file writes. `q`, `Esc`, and `Ctrl+C` stop TUI mode.
- `src/ui/graph.rs`, `table.rs`, `point.rs`, and `sparkline.rs` are independent
  renderers. View state is defined in `src/view.rs` and switched with `1`-`4` or
  `Tab`; theme state is managed by `src/ui/theme.rs` and switched with `t`.
- `src/terminal.rs` and the guards in `src/main.rs` must continue to restore the
  terminal on normal and error exits.

When adding a view, update the `View` enum and its conversion/name/rotation
methods, register the renderer in `src/ui/mod.rs`, add its dispatch arm in
`src/draw.rs`, and add its keyboard binding to the event loop.

## Exporter Architecture

Exporter mode is implemented by `src/main.rs` and `src/exporter/`:

- `exporter/runner.rs` starts one OS thread per target and updates metrics directly
  for either ICMP results or TCP connection attempts.
- `exporter/metric.rs` owns the Prometheus registry and the Hyper HTTP server.
- `GET /metrics` returns Prometheus text; `GET /` returns a small status page;
  other routes return 404.
- Shutdown is coordinated through the shared atomic running flag and a Tokio
  oneshot signal. `Ctrl+C`, `q`, and `Esc` must allow the HTTP server and workers
  to exit cleanly.

Metric names and labels are a public scrape contract. Preserve these unless the
change explicitly includes its downstream impact:

- `nbping_ping_duration_seconds{target,ip}`
- `nbping_ping_requests_total{target,ip,status}` where status is `success`,
  `timeout`, or `error`

## Change Discipline

- Keep changes scoped to the affected module and follow the existing Rust style.
- Preserve existing comments and behavior outside the requested change.
- Reuse the current channel, state, configuration, and rendering patterns before
  introducing new abstractions.
- Account for both TUI and exporter callers when changing `network` or config code.
- When changing `IpData`, update initialization, data processing, and every renderer.
- Validate user-facing behavior at startup where practical, especially config enum
  values and numeric ranges.

## Verification

Run the narrowest relevant checks after changes. For normal code changes, run:

```text
cargo test
```

For CLI, config, HTTP, or rendering changes, also run a focused manual command or
targeted test when the environment supports it. Report any checks that could not
be run and distinguish build/test verification from live network or terminal
behavior.
