//! YAML configuration file support.
//!
//! `nbping` can be started either from command-line flags or from a YAML file
//! passed via `--config`. The fields here mirror the CLI flags one-to-one
//! (a flat schema). Every field is optional so that command-line arguments can
//! selectively override the file: the resolution order is
//! `CLI explicit flag > YAML config > built-in default`.
//!
//! See `nbping.example.yaml` in the repository root for a documented sample.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::view::View;

/// A configuration file deserialized from YAML.
///
/// All fields are `Option` so an absent field falls through to the next layer
/// (CLI default or built-in default) instead of clobbering it. `deny_unknown_fields`
/// turns typos into hard errors rather than silently ignored keys.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    /// Execution mode: `tui` (default) or `exporter`.
    pub mode: Option<String>,
    /// Target IP addresses or hostnames to ping.
    pub targets: Option<Vec<String>>,
    /// Number of pings to send (0 = unlimited).
    pub count: Option<usize>,
    /// Interval between pings, in seconds.
    pub interval: Option<i32>,
    /// Force using IPv6.
    pub force_ipv6: Option<bool>,
    /// Resolve multiple A/AAAA records for a single target (tui mode only).
    pub multiple: Option<i32>,
    /// Initial view: graph/table/point/sparkline (tui mode only).
    pub view_type: Option<String>,
    /// File to save ping results to (tui mode only).
    pub output: Option<String>,
    /// Prometheus metrics HTTP port (exporter mode only).
    pub port: Option<u16>,
}

/// Execution mode selected by the config file's `mode` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Tui,
    Exporter,
}

impl FileConfig {
    /// Read and parse a YAML config file, then validate its contents.
    ///
    /// Returns a friendly error (with the file path) when the file is missing,
    /// malformed, or contains invalid values.
    pub fn load(path: &str) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path))?;
        let config: FileConfig = serde_yaml_ng::from_str(&contents)
            .with_context(|| format!("failed to parse config file: {}", path))?;
        config.validate()?;
        Ok(config)
    }

    /// Validate enum-like string fields and numeric ranges up front so errors
    /// surface at startup rather than deep inside the run path.
    fn validate(&self) -> Result<()> {
        if let Some(mode) = &self.mode {
            self.parsed_mode_inner(mode)?;
        }
        if let Some(view) = &self.view_type {
            if View::from_str(view).is_none() {
                return Err(anyhow!(
                    "invalid view_type '{}' in config (expected one of: graph, table, point, sparkline)",
                    view
                ));
            }
        }
        // Negative durations/counts are nonsensical and would produce negative
        // millisecond sleeps downstream. (`count` is a `usize`, so serde already
        // rejects negatives for it.)
        if let Some(i) = self.interval {
            if i < 0 {
                return Err(anyhow!("interval must be >= 0, got {}", i));
            }
            if i > 86400 {
                return Err(anyhow!("interval must be <= 86400 (24 h), got {}", i));
            }
        }
        if let Some(m) = self.multiple {
            if m < 0 {
                return Err(anyhow!("multiple must be >= 0, got {}", m));
            }
        }
        Ok(())
    }

    /// The execution mode declared by the file, defaulting to `Tui` when absent.
    pub fn mode(&self) -> Result<Mode> {
        match &self.mode {
            Some(m) => self.parsed_mode_inner(m),
            None => Ok(Mode::Tui),
        }
    }

    fn parsed_mode_inner(&self, mode: &str) -> Result<Mode> {
        match mode {
            "tui" => Ok(Mode::Tui),
            "exporter" => Ok(Mode::Exporter),
            other => Err(anyhow!(
                "invalid mode '{}' in config (expected 'tui' or 'exporter')",
                other
            )),
        }
    }
}

/// Fully-resolved settings for the default TUI mode, after merging
/// `CLI > YAML > default`.
#[derive(Debug, PartialEq)]
pub struct ResolvedTui {
    pub targets: Vec<String>,
    pub count: usize,
    pub interval: i32,
    pub force_ipv6: bool,
    pub multiple: i32,
    pub view_type: String,
    pub output: Option<String>,
}

/// Fully-resolved settings for exporter mode, after merging `CLI > YAML > default`.
#[derive(Debug, PartialEq)]
pub struct ResolvedExporter {
    pub targets: Vec<String>,
    pub interval: i32,
    pub port: u16,
    pub force_ipv6: bool,
}

/// Resolve the target list: CLI targets win when present, otherwise fall back to
/// the config file's `targets`. The result is de-duplicated while preserving the
/// original order.
pub fn resolve_targets(cli_targets: Vec<String>, file: &FileConfig) -> Vec<String> {
    let raw = if !cli_targets.is_empty() {
        cli_targets
    } else {
        file.targets.clone().unwrap_or_default()
    };

    let mut seen = std::collections::HashSet::new();
    raw.into_iter().filter(|item| seen.insert(item.clone())).collect()
}

/// Merge CLI flags and config-file values into the final TUI settings.
/// Precedence per field: CLI explicit value > YAML config > built-in default.
pub fn resolve_tui(
    cli_targets: Vec<String>,
    cli_count: Option<usize>,
    cli_interval: Option<i32>,
    cli_force_ipv6: bool,
    cli_multiple: Option<i32>,
    cli_view_type: Option<String>,
    cli_output: Option<String>,
    file: &FileConfig,
) -> ResolvedTui {
    ResolvedTui {
        targets: resolve_targets(cli_targets, file),
        count: cli_count.or(file.count).unwrap_or(0),
        // 0 is meaningful in TUI mode (run_app treats it as 500ms).
        interval: cli_interval.or(file.interval).unwrap_or(0),
        // A bool flag can only be turned ON from the CLI; the config can also enable it.
        force_ipv6: cli_force_ipv6 || file.force_ipv6.unwrap_or(false),
        multiple: cli_multiple.or(file.multiple).unwrap_or(0),
        view_type: cli_view_type
            .or_else(|| file.view_type.clone())
            .unwrap_or_else(|| "graph".to_string()),
        output: cli_output.or_else(|| file.output.clone()),
    }
}

/// Merge CLI flags and config-file values into the final exporter settings.
/// Exporter mode is reachable two ways, so values from the `exporter` subcommand
/// take precedence over top-level flags, then the config file, then defaults.
pub fn resolve_exporter(
    sub_targets: Vec<String>,
    sub_interval: Option<i32>,
    sub_port: Option<u16>,
    top_targets: Vec<String>,
    top_interval: Option<i32>,
    cli_force_ipv6: bool,
    file: &FileConfig,
) -> ResolvedExporter {
    let cli_targets = if !sub_targets.is_empty() {
        sub_targets
    } else {
        top_targets
    };
    let interval = sub_interval.or(top_interval).or(file.interval).unwrap_or(1);
    // Unlike TUI mode, exporter mode has no "0 == 500ms" semantics; a zero or
    // negative interval would busy-loop, so fall back to 1 second.
    let interval = if interval <= 0 { 1 } else { interval };
    ResolvedExporter {
        targets: resolve_targets(cli_targets, file),
        interval,
        port: sub_port.or(file.port).unwrap_or(9090),
        force_ipv6: cli_force_ipv6 || file.force_ipv6.unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let yaml = r#"
mode: exporter
targets:
  - google.com
  - 1.1.1.1
count: 5
interval: 2
force_ipv6: true
multiple: 3
view_type: table
output: out.log
port: 9100
"#;
        let cfg: FileConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(cfg.mode.as_deref(), Some("exporter"));
        assert_eq!(
            cfg.targets,
            Some(vec!["google.com".to_string(), "1.1.1.1".to_string()])
        );
        assert_eq!(cfg.count, Some(5));
        assert_eq!(cfg.interval, Some(2));
        assert_eq!(cfg.force_ipv6, Some(true));
        assert_eq!(cfg.multiple, Some(3));
        assert_eq!(cfg.view_type.as_deref(), Some("table"));
        assert_eq!(cfg.output.as_deref(), Some("out.log"));
        assert_eq!(cfg.port, Some(9100));
        assert_eq!(cfg.mode().unwrap(), Mode::Exporter);
    }

    #[test]
    fn empty_config_defaults_to_tui() {
        let cfg: FileConfig = serde_yaml_ng::from_str("targets: [a.com]").unwrap();
        assert_eq!(cfg.mode().unwrap(), Mode::Tui);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn rejects_unknown_fields() {
        // A typo'd key must be a hard error, not silently dropped.
        let err = serde_yaml_ng::from_str::<FileConfig>("intervall: 5").unwrap_err();
        assert!(err.to_string().contains("unknown field"), "{}", err);
    }

    #[test]
    fn rejects_invalid_mode() {
        let cfg: FileConfig = serde_yaml_ng::from_str("mode: bogus").unwrap();
        assert!(cfg.validate().is_err());
        assert!(cfg.mode().is_err());
    }

    #[test]
    fn rejects_invalid_view_type() {
        let cfg: FileConfig = serde_yaml_ng::from_str("view_type: pie").unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn valid_view_types_accepted() {
        for v in ["graph", "table", "point", "sparkline"] {
            let cfg: FileConfig =
                serde_yaml_ng::from_str(&format!("view_type: {}", v)).unwrap();
            assert!(cfg.validate().is_ok(), "{} should be valid", v);
        }
    }

    #[test]
    fn rejects_negative_numbers() {
        let cfg: FileConfig = serde_yaml_ng::from_str("interval: -5").unwrap();
        assert!(cfg.validate().is_err());
        let cfg: FileConfig = serde_yaml_ng::from_str("multiple: -1").unwrap();
        assert!(cfg.validate().is_err());
        // serde rejects a negative `count` (usize) before validate() even runs.
        assert!(serde_yaml_ng::from_str::<FileConfig>("count: -1").is_err());
    }

    #[test]
    fn rejects_interval_over_86400() {
        // Values this large would overflow i32 after *1000 and produce an
        // enormous sleep duration in the ping worker.
        let cfg: FileConfig = serde_yaml_ng::from_str("interval: 86401").unwrap();
        assert!(cfg.validate().is_err());
        // Edge: exactly 86400 is the allowed maximum.
        let cfg: FileConfig = serde_yaml_ng::from_str("interval: 86400").unwrap();
        assert!(cfg.validate().is_ok());
    }

    fn file(yaml: &str) -> FileConfig {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    // ---- TUI merge precedence ----

    #[test]
    fn tui_cli_overrides_file() {
        let f = file("interval: 9\ncount: 100\nview_type: table");
        let r = resolve_tui(
            vec!["a.com".into()],
            Some(5),           // cli count
            Some(2),           // cli interval
            false,
            None,
            Some("point".into()), // cli view_type
            None,
            &f,
        );
        assert_eq!(r.count, 5);
        assert_eq!(r.interval, 2);
        assert_eq!(r.view_type, "point");
        assert_eq!(r.targets, vec!["a.com".to_string()]);
    }

    #[test]
    fn tui_falls_back_to_file_then_default() {
        let f = file("interval: 9\nview_type: sparkline");
        let r = resolve_tui(vec!["a.com".into()], None, None, false, None, None, None, &f);
        assert_eq!(r.interval, 9); // from file
        assert_eq!(r.view_type, "sparkline"); // from file
        assert_eq!(r.count, 0); // default

        let empty = FileConfig::default();
        let r = resolve_tui(vec!["a.com".into()], None, None, false, None, None, None, &empty);
        assert_eq!(r.interval, 0); // tui default
        assert_eq!(r.view_type, "graph"); // default
        assert_eq!(r.multiple, 0);
    }

    #[test]
    fn tui_force_ipv6_cli_or_file() {
        let on = file("force_ipv6: true");
        let off = FileConfig::default();
        // CLI flag enables it.
        assert!(resolve_tui(vec!["a".into()], None, None, true, None, None, None, &off).force_ipv6);
        // File enables it even without the CLI flag.
        assert!(resolve_tui(vec!["a".into()], None, None, false, None, None, None, &on).force_ipv6);
        // Neither set -> off.
        assert!(!resolve_tui(vec!["a".into()], None, None, false, None, None, None, &off).force_ipv6);
    }

    // ---- targets resolution ----

    #[test]
    fn cli_targets_win_over_file() {
        let f = file("targets: [x.com, y.com]");
        let r = resolve_targets(vec!["a.com".into(), "a.com".into(), "b.com".into()], &f);
        // CLI wins, and is de-duplicated while preserving order.
        assert_eq!(r, vec!["a.com".to_string(), "b.com".to_string()]);
    }

    #[test]
    fn empty_cli_targets_fall_back_to_file() {
        let f = file("targets: [x.com, y.com, x.com]");
        let r = resolve_targets(vec![], &f);
        assert_eq!(r, vec!["x.com".to_string(), "y.com".to_string()]);
    }

    // ---- exporter merge precedence (two entry paths) ----

    #[test]
    fn exporter_defaults_match_legacy() {
        // No CLI, no file -> exporter interval defaults to 1, port to 9090.
        let r = resolve_exporter(vec!["a".into()], None, None, vec![], None, false, &FileConfig::default());
        assert_eq!(r.interval, 1);
        assert_eq!(r.port, 9090);
    }

    #[test]
    fn exporter_subcommand_beats_toplevel_and_file() {
        let f = file("interval: 9\nport: 8000");
        // subcommand -i 2, top-level -i 3, file 9 -> subcommand wins.
        let r = resolve_exporter(vec!["a".into()], Some(2), Some(9100), vec![], Some(3), false, &f);
        assert_eq!(r.interval, 2);
        assert_eq!(r.port, 9100);
    }

    #[test]
    fn exporter_toplevel_interval_used_when_no_subcommand() {
        // mode:exporter via config, user passes top-level -i 3; subcommand absent.
        let f = file("interval: 9");
        let r = resolve_exporter(vec![], None, None, vec!["a".into()], Some(3), false, &f);
        assert_eq!(r.interval, 3);
        assert_eq!(r.targets, vec!["a".to_string()]);
    }

    #[test]
    fn exporter_zero_interval_is_guarded() {
        // A 0 interval (no "500ms" meaning in exporter mode) must not busy-loop.
        let f = file("interval: 0");
        let r = resolve_exporter(vec!["a".into()], None, None, vec![], None, false, &f);
        assert_eq!(r.interval, 1);
    }

    #[test]
    fn exporter_force_ipv6_honored() {
        let f = file("force_ipv6: true");
        // Now actually plumbed through (regression guard for the B1 fix).
        assert!(resolve_exporter(vec!["a".into()], None, None, vec![], None, false, &f).force_ipv6);
        assert!(resolve_exporter(vec!["a".into()], None, None, vec![], None, true, &FileConfig::default()).force_ipv6);
    }
}
