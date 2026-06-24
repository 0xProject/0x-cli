//! Anonymous, opt-out usage telemetry via Amplitude's HTTP V2 API.
//!
//! Design goals: never delay the user, never leak sensitive data, and be
//! trivially disableable.
//!
//! - **Inert unless a key is compiled in.** [`AMPLITUDE_API_KEY`] comes from
//!   `option_env!`, so dev and CI builds have no key and this module does
//!   nothing — tests can't accidentally phone home.
//! - **Opt-out, three ways.** `0x config set telemetry.enabled false`, the
//!   `ZEROEX_TELEMETRY` env var (falsy), or the cross-tool `DO_NOT_TRACK`
//!   standard. Resolution is the pure [`telemetry_allowed`] function.
//! - **Hybrid delivery.** Events spool to `~/.0x-config/telemetry-queue.jsonl`.
//!   [`init`] spawns a background flush of the backlog that overlaps the
//!   current command's own work; [`Telemetry::record`] appends the new event
//!   and does a 300ms best-effort flush at exit. The user waits ≤300ms, and
//!   usually 0. A per-event `insert_id` lets Amplitude dedupe the two paths.
//! - **Privacy allow-list.** [`CommandEvent`] is the *only* shape sent as
//!   `event_properties`; the `privacy_snapshot` test pins its exact fields.
//!   Never: addresses, amounts, hashes, keys, RPC URLs, error messages, or IP.

use crate::cli::{Cli, Commands, OutputFormat};
use crate::config;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Amplitude project key, compiled in at release-build time via the
/// `ZEROEX_AMPLITUDE_API_KEY` build env var. `None` in any build that didn't
/// set it (all dev/CI/test builds) → telemetry is fully inert.
const AMPLITUDE_API_KEY: Option<&str> = option_env!("ZEROEX_AMPLITUDE_API_KEY");

const DEFAULT_AMPLITUDE_URL: &str = "https://api2.amplitude.com/2/httpapi";
const EVENT_TYPE: &str = "cli_command";
const QUEUE_FILE: &str = "telemetry-queue.jsonl";
const MAX_QUEUE_EVENTS: usize = 50;
const PIGGYBACK_TIMEOUT: Duration = Duration::from_secs(3);
const EXIT_FLUSH_TIMEOUT: Duration = Duration::from_millis(300);

/// Inputs to the opt-out decision, isolated so the precedence logic can be
/// unit-tested without touching process env or the compiled-in key.
struct OptOutInputs {
    compiled_key: Option<&'static str>,
    zerox_telemetry: Option<String>,
    do_not_track: Option<String>,
    config_enabled: bool,
}

/// Whether telemetry may run this invocation. Order: no compiled key → off;
/// `DO_NOT_TRACK` set to anything other than empty/`0` → off; `ZEROEX_TELEMETRY`
/// falsy → off; config `telemetry.enabled == false` → off.
fn telemetry_allowed(i: &OptOutInputs) -> bool {
    if i.compiled_key.is_none() {
        return false;
    }
    if i
        .do_not_track
        .as_deref()
        .is_some_and(|v| !v.is_empty() && v != "0")
    {
        return false;
    }
    if i.zerox_telemetry.as_deref().is_some_and(is_falsy) {
        return false;
    }
    i.config_enabled
}

fn is_falsy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off" | ""
    )
}

/// A live telemetry handle. Held by `main` for the duration of a command and
/// consumed by [`Telemetry::record`] at exit. Absent (`None` from [`init`])
/// whenever telemetry is disabled.
pub struct Telemetry {
    inner: Arc<Inner>,
}

struct Inner {
    api_key: &'static str,
    url: String,
    /// Opaque per-install token (random UUID). Sent as Amplitude's required
    /// `device_id` field, but it is not a device identifier.
    install_id: String,
    client: reqwest::Client,
    queue_path: PathBuf,
    /// Serializes file access between the background flush and the exit flush
    /// so a read-modify-write can't clobber a concurrent append. Held only for
    /// file IO, never across the network request.
    lock: tokio::sync::Mutex<()>,
}

/// Initialize telemetry for this invocation. Returns `None` (and does nothing)
/// when telemetry is disabled by any opt-out, when no key is compiled in, or
/// when config can't be read/persisted. On first activation, generates the
/// install token, persists it, and prints a one-time stderr notice.
pub fn init() -> Option<Telemetry> {
    // Telemetry must never take down an otherwise-fine command. `config_dir()`
    // (used by the config load and the queue path below) `.expect()`s a home
    // directory; if none can be resolved, silently disable telemetry instead
    // of panicking a command — like `0x chains` — that needs no config at all.
    dirs::home_dir()?;

    // Disk-only view: we may persist it below, and the env-overlaid view from
    // load_config() would leak env-derived secrets onto disk.
    let mut config = config::load_config_disk_only().ok()?;

    let inputs = OptOutInputs {
        compiled_key: AMPLITUDE_API_KEY,
        zerox_telemetry: std::env::var("ZEROEX_TELEMETRY").ok(),
        do_not_track: std::env::var("DO_NOT_TRACK").ok(),
        config_enabled: config.telemetry.enabled,
    };
    if !telemetry_allowed(&inputs) {
        return None;
    }
    let api_key = AMPLITUDE_API_KEY?; // Some, guaranteed by telemetry_allowed

    // Resolve (or mint) the install token. Minting persists it and shows the
    // first-run notice exactly once.
    let install_id = match config.telemetry.install_id.clone() {
        Some(id) => id,
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            config.telemetry.install_id = Some(id.clone());
            // If we can't persist the token (e.g. read-only FS), don't track —
            // better silent-off than re-minting an id on every run or crashing.
            if config::save_config(&config).is_err() {
                return None;
            }
            print_first_run_notice();
            id
        }
    };

    let client = reqwest::Client::builder().build().ok()?;
    let inner = Arc::new(Inner {
        api_key,
        url: amplitude_url(),
        install_id,
        client,
        queue_path: config::config_dir().join(QUEUE_FILE),
        lock: tokio::sync::Mutex::new(()),
    });

    // Piggyback: drain the backlog in the background, overlapping the command's
    // own network work. If the process exits first the task is dropped — the
    // queue is intact on disk and resends next run (insert_id dedupes).
    let backlog = inner.clone();
    tokio::spawn(async move {
        flush(&backlog, PIGGYBACK_TIMEOUT).await;
    });

    Some(Telemetry { inner })
}

impl Telemetry {
    /// Enqueue one event and do a bounded best-effort flush. Never delays exit
    /// by more than [`EXIT_FLUSH_TIMEOUT`]; every failure path is silent
    /// (debug-logged).
    pub async fn record(self, event: CommandEvent) {
        let Some(line) = build_event_line(&self.inner, event) else {
            return;
        };
        {
            let _guard = self.inner.lock.lock().await;
            if let Err(e) = append_capped(&self.inner.queue_path, &line) {
                tracing::debug!("telemetry: failed to enqueue event: {e}");
                return;
            }
        }
        // Hard ceiling on exit latency, independent of the request timeout.
        let _ = tokio::time::timeout(EXIT_FLUSH_TIMEOUT, flush(&self.inner, EXIT_FLUSH_TIMEOUT))
            .await;
    }
}

/// The privacy allow-list: the only fields ever sent as `event_properties`.
/// Pinned by the `privacy_snapshot` test — adding a field here without updating
/// that test (and consciously deciding it's safe) fails the build.
#[derive(Debug, Clone, Serialize)]
pub struct CommandEvent {
    /// Stable command name (`Commands::name()`), e.g. "swap", "cross-chain".
    pub command: &'static str,
    pub exit_code: i32,
    /// Stable `ErrorCode::as_str()` on failure, else absent. Never the error
    /// *message* — messages can embed addresses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<&'static str>,
    pub duration_ms: u64,
    /// Chain *name* only (e.g. "base"); never token addresses. Absent for
    /// commands without a chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,
    pub gasless: bool,
    pub dry_run: bool,
    pub output_format: &'static str,
    /// Whether the `CI` env var is set — tags CI runs rather than dropping them.
    pub ci: bool,
}

impl CommandEvent {
    /// Build the event from the parsed CLI plus the command's outcome. Only
    /// pulls chain/gasless from the typed args — no free-form value ever flows
    /// in, which is what keeps the allow-list honest.
    pub fn from_cli(
        cli: &Cli,
        exit_code: i32,
        error_code: Option<&'static str>,
        duration: Duration,
        format: OutputFormat,
    ) -> Self {
        let (chain, gasless) = match &cli.command {
            Commands::Price(a) => (Some(a.chain.clone()), a.gasless),
            Commands::Swap(a) => (Some(a.chain.clone()), a.gasless),
            // Origin chain stands in for "the chain" on a bridge.
            Commands::CrossChain(a) => (Some(a.from.clone()), false),
            Commands::Status(a) => (a.chain.clone(), false),
            _ => (None, false),
        };
        let output_format = match format {
            OutputFormat::Human => "human",
            OutputFormat::Json => "json",
            OutputFormat::JsonEnvelope => "json-envelope",
        };
        Self {
            command: cli.command.name(),
            exit_code,
            error_code,
            duration_ms: duration.as_millis() as u64,
            chain,
            gasless,
            dry_run: cli.dry_run,
            output_format,
            ci: std::env::var_os("CI").is_some(),
        }
    }
}

/// One Amplitude event as sent on the wire. `device_id` carries our opaque
/// install token — Amplitude requires the field name, but the value is not a
/// device identifier.
#[derive(Serialize)]
struct AmplitudeEvent<'a> {
    device_id: &'a str,
    event_type: &'a str,
    time: i64,
    insert_id: String,
    app_version: &'a str,
    os_name: &'a str,
    platform: &'a str,
    event_properties: CommandEvent,
}

fn build_event_line(inner: &Inner, event: CommandEvent) -> Option<String> {
    let wire = AmplitudeEvent {
        device_id: &inner.install_id,
        event_type: EVENT_TYPE,
        time: chrono::Utc::now().timestamp_millis(),
        insert_id: uuid::Uuid::new_v4().to_string(),
        app_version: env!("CARGO_PKG_VERSION"),
        os_name: std::env::consts::OS,
        platform: std::env::consts::ARCH,
        event_properties: event,
    };
    serde_json::to_string(&wire).ok()
}

fn amplitude_url() -> String {
    // Internal/testing override (point at a mock or unroutable host). Not
    // documented publicly.
    std::env::var("ZEROEX_AMPLITUDE_URL").unwrap_or_else(|_| DEFAULT_AMPLITUDE_URL.to_string())
}

fn print_first_run_notice() {
    eprintln!(
        "0x collects anonymous usage stats (command, exit code, duration — never addresses, \
         amounts, or keys) to improve the CLI. Opt out any time: \
         `0x config set telemetry.enabled false`, or set DO_NOT_TRACK=1. This notice is shown once."
    );
}

/// Read the backlog, POST it as one batch, and on success prune exactly the
/// events that were sent (by `insert_id`) — never a blind truncate, so events
/// appended during the request are preserved. The network request runs without
/// the file lock held, so two concurrent flushes can at worst double-send
/// (Amplitude dedupes on `insert_id`); they can't lose data.
async fn flush(inner: &Inner, request_timeout: Duration) {
    let lines = {
        let _guard = inner.lock.lock().await;
        read_lines(&inner.queue_path)
    };
    if lines.is_empty() {
        return;
    }

    let mut events = Vec::new();
    let mut sent_ids = Vec::new();
    for line in &lines {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(id) = value.get("insert_id").and_then(|v| v.as_str()) {
                sent_ids.push(id.to_string());
                events.push(value);
            }
        }
    }

    if events.is_empty() {
        // Queue is all garbage — prune it (remove_sent with no ids drops
        // anything unparseable) so it can't grow unbounded.
        let _guard = inner.lock.lock().await;
        let _ = remove_sent(&inner.queue_path, &[]);
        return;
    }

    let payload = serde_json::json!({ "api_key": inner.api_key, "events": events });
    let resp = inner
        .client
        .post(&inner.url)
        .json(&payload)
        .timeout(request_timeout)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let _guard = inner.lock.lock().await;
            if let Err(e) = remove_sent(&inner.queue_path, &sent_ids) {
                tracing::debug!("telemetry: failed to prune queue: {e}");
            }
        }
        Ok(r) => tracing::debug!("telemetry: Amplitude returned status {}", r.status()),
        Err(e) => tracing::debug!("telemetry: send failed: {e}"),
    }
}

fn read_lines(path: &Path) -> Vec<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => s
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(String::from)
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn write_lines(path: &Path, lines: &[String]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = lines.join("\n");
    body.push('\n');

    // Write to a per-process temp file then rename. Rename is atomic on the
    // same filesystem, so a crash mid-write — or a concurrent `0x` process —
    // can never leave a half-written queue that fails to parse. The pid suffix
    // keeps concurrent processes from colliding on the same temp path. Within a
    // single process every queue write happens under `Inner::lock`, so our own
    // temp can't collide with itself.
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(QUEUE_FILE);
    let tmp = path.with_file_name(format!("{file_name}.tmp.{}", std::process::id()));
    std::fs::write(&tmp, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path)
}

fn append_capped(path: &Path, line: &str) -> std::io::Result<()> {
    let mut lines = read_lines(path);
    lines.push(line.to_string());
    if lines.len() > MAX_QUEUE_EVENTS {
        let drop = lines.len() - MAX_QUEUE_EVENTS;
        lines.drain(0..drop);
    }
    write_lines(path, &lines)
}

/// Rewrite the queue keeping only well-formed events whose `insert_id` is not
/// in `sent_ids`. Drops unparseable/id-less lines too. Removes the file when
/// nothing remains.
fn remove_sent(path: &Path, sent_ids: &[String]) -> std::io::Result<()> {
    let kept: Vec<String> = read_lines(path)
        .into_iter()
        .filter(|line| match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => v
                .get("insert_id")
                .and_then(|x| x.as_str())
                .is_some_and(|id| !sent_ids.iter().any(|s| s == id)),
            Err(_) => false,
        })
        .collect();
    if kept.is_empty() {
        let _ = std::fs::remove_file(path);
        Ok(())
    } else {
        write_lines(path, &kept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn inputs(
        key: Option<&'static str>,
        zerox: Option<&str>,
        dnt: Option<&str>,
        enabled: bool,
    ) -> OptOutInputs {
        OptOutInputs {
            compiled_key: key,
            zerox_telemetry: zerox.map(String::from),
            do_not_track: dnt.map(String::from),
            config_enabled: enabled,
        }
    }

    #[test]
    fn no_compiled_key_disables_everything() {
        assert!(!telemetry_allowed(&inputs(None, None, None, true)));
    }

    #[test]
    fn do_not_track_wins_over_enabled_config() {
        assert!(!telemetry_allowed(&inputs(Some("k"), None, Some("1"), true)));
        // The standard: any value other than empty/"0" means opt out.
        assert!(!telemetry_allowed(&inputs(Some("k"), None, Some("true"), true)));
        // DO_NOT_TRACK=0 or empty does NOT opt out.
        assert!(telemetry_allowed(&inputs(Some("k"), None, Some("0"), true)));
        assert!(telemetry_allowed(&inputs(Some("k"), None, Some(""), true)));
    }

    #[test]
    fn zerox_telemetry_falsy_disables() {
        for v in ["0", "false", "no", "off", "FALSE", "Off", ""] {
            assert!(
                !telemetry_allowed(&inputs(Some("k"), Some(v), None, true)),
                "expected {v:?} to disable"
            );
        }
        // Truthy / unset leaves it enabled.
        assert!(telemetry_allowed(&inputs(Some("k"), Some("1"), None, true)));
        assert!(telemetry_allowed(&inputs(Some("k"), None, None, true)));
    }

    #[test]
    fn config_disabled_disables() {
        assert!(!telemetry_allowed(&inputs(Some("k"), None, None, false)));
    }

    #[test]
    fn all_clear_is_allowed() {
        assert!(telemetry_allowed(&inputs(Some("k"), None, None, true)));
    }

    /// The privacy contract. If this set ever changes, it must be a conscious
    /// decision: confirm the new field carries nothing sensitive.
    #[test]
    fn privacy_snapshot_event_properties_keys() {
        let event = CommandEvent {
            command: "swap",
            exit_code: 6,
            error_code: Some("INSUFFICIENT_BALANCE"),
            duration_ms: 1234,
            chain: Some("base".into()),
            gasless: true,
            dry_run: false,
            output_format: "json-envelope",
            ci: true,
        };
        let value = serde_json::to_value(&event).unwrap();
        let keys: BTreeSet<&str> = value.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        let expected: BTreeSet<&str> = [
            "command",
            "exit_code",
            "error_code",
            "duration_ms",
            "chain",
            "gasless",
            "dry_run",
            "output_format",
            "ci",
        ]
        .into_iter()
        .collect();
        assert_eq!(keys, expected, "event_properties allow-list changed");
    }

    /// The wire envelope around the event: also an allow-list.
    #[test]
    fn privacy_snapshot_wire_event_keys() {
        let event = CommandEvent {
            command: "price",
            exit_code: 0,
            error_code: None,
            duration_ms: 10,
            chain: None,
            gasless: false,
            dry_run: false,
            output_format: "human",
            ci: false,
        };
        let wire = AmplitudeEvent {
            device_id: "install-token",
            event_type: EVENT_TYPE,
            time: 1_700_000_000_000,
            insert_id: "insert-uuid".into(),
            app_version: "0.0.0",
            os_name: "linux",
            platform: "x86_64",
            event_properties: event,
        };
        let value = serde_json::to_value(&wire).unwrap();
        let keys: BTreeSet<&str> = value.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        let expected: BTreeSet<&str> = [
            "device_id",
            "event_type",
            "time",
            "insert_id",
            "app_version",
            "os_name",
            "platform",
            "event_properties",
        ]
        .into_iter()
        .collect();
        assert_eq!(keys, expected, "wire event allow-list changed");
        // Sanity: optional fields are omitted, not null.
        assert!(value["event_properties"].get("error_code").is_none());
        assert!(value["event_properties"].get("chain").is_none());
    }

    fn line_with_id(id: &str) -> String {
        serde_json::json!({ "insert_id": id, "event_type": "cli_command" }).to_string()
    }

    #[test]
    fn queue_appends_and_caps_at_max() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.jsonl");
        for i in 0..(MAX_QUEUE_EVENTS + 10) {
            append_capped(&path, &line_with_id(&format!("id-{i}"))).unwrap();
        }
        let lines = read_lines(&path);
        assert_eq!(lines.len(), MAX_QUEUE_EVENTS);
        // Oldest dropped: the first surviving id is the 10th appended.
        assert!(lines[0].contains("id-10"));
        assert!(lines.last().unwrap().contains(&format!("id-{}", MAX_QUEUE_EVENTS + 9)));
    }

    #[test]
    fn remove_sent_prunes_only_sent_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.jsonl");
        append_capped(&path, &line_with_id("a")).unwrap();
        append_capped(&path, &line_with_id("b")).unwrap();
        append_capped(&path, &line_with_id("c")).unwrap();
        remove_sent(&path, &["a".to_string(), "c".to_string()]).unwrap();
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("\"b\""));
    }

    #[test]
    fn remove_sent_drops_garbage_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.jsonl");
        write_lines(
            &path,
            &["not json".to_string(), line_with_id("keep")],
        )
        .unwrap();
        // Pruning with no sent ids should still drop the unparseable line.
        remove_sent(&path, &[]).unwrap();
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("keep"));
    }

    #[test]
    fn remove_sent_removes_file_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.jsonl");
        append_capped(&path, &line_with_id("only")).unwrap();
        remove_sent(&path, &["only".to_string()]).unwrap();
        assert!(!path.exists());
    }
}
