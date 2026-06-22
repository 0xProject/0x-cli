use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn cmd() -> Command {
    Command::cargo_bin("0x").unwrap()
}

/// A binary invocation rooted in a freshly-minted `HOME` directory. Use this
/// for tests that assert on "no API key" / "no wallet" so they aren't fooled
/// by the developer's real `~/.0x-config`. The returned `TempDir` must be
/// kept in scope until the command runs, otherwise the directory is dropped
/// and the binary will create a fresh empty config in a stale path.
fn cmd_in_temp_home() -> (Command, TempDir) {
    let tmp = TempDir::new().expect("temp home");
    let mut c = Command::cargo_bin("0x").unwrap();
    c.env("HOME", tmp.path());
    (c, tmp)
}

// ─── Help & Version ───────────────────────────────────────────

#[test]
fn test_help_exits_0() {
    cmd().arg("--help").assert().success().stdout(
        predicate::str::contains("0x Protocol")
            .and(predicate::str::contains("Commands:"))
            .and(predicate::str::contains("EXIT CODES")),
    );
}

#[test]
fn test_version() {
    cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("0x 0.1.0"));
}

#[test]
fn test_swap_help_has_examples() {
    cmd()
        .args(["swap", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("EXAMPLES"));
}

#[test]
fn test_cross_chain_help_has_examples() {
    cmd()
        .args(["cross-chain", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("EXAMPLES"));
}

#[test]
fn test_config_set_help() {
    cmd()
        .args(["config", "set", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("KEYS:"));
}

// ─── Chains ───────────────────────────────────────────────────

#[test]
fn test_chains_human() {
    cmd()
        .args(["chains", "-o", "human"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("ethereum")
                .and(predicate::str::contains("base"))
                .and(predicate::str::contains("solana")),
        );
}

#[test]
fn test_chains_json_is_valid() {
    let output = cmd()
        .args(["chains", "-o", "json"])
        .output()
        .expect("failed to run");
    assert!(output.status.success());

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("invalid JSON output");
    assert!(json.is_array());

    let chains = json.as_array().unwrap();
    assert!(chains.len() >= 10, "Expected at least 10 chains");

    // Verify first chain has expected fields
    let first = &chains[0];
    assert!(first.get("name").is_some());
    assert!(first.get("native_token").is_some());
    assert!(first.get("chain_type").is_some());
    // EVM chain ids must be JSON numbers per the documented contract.
    assert!(
        first["id"].is_u64(),
        "EVM chain id should be a JSON number, got {}",
        first["id"]
    );

    // Solana entry must serialize id as the string "solana" — pre-pass-3 it
    // was `null` because of an `#[serde(untagged)]` unit variant.
    let solana = chains
        .iter()
        .find(|c| c["name"] == "solana")
        .expect("solana entry");
    assert_eq!(solana["id"], "solana");
}

#[test]
fn test_chains_json_envelope() {
    let output = cmd()
        .args(["chains", "-o", "json-envelope"])
        .output()
        .expect("failed to run");
    assert!(output.status.success());

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("invalid JSON envelope");

    assert_eq!(json["version"], "1");
    assert_eq!(json["status"], "success");
    assert_eq!(json["command"], "chains");
    assert!(json["data"].is_array());
    assert!(json["warnings"].is_array());
    assert!(json["metadata"].is_object());
    // exit_code in envelope must match process exit code (here, 0).
    assert_eq!(json["exit_code"], 0);
}

// ─── Error Handling ───────────────────────────────────────────

#[test]
fn test_swap_without_required_args_exits_2() {
    cmd().arg("swap").assert().failure().code(2);
}

#[test]
fn test_unknown_subcommand_exits_2() {
    cmd().arg("nonexistent").assert().failure().code(2);
}

#[test]
fn test_swap_unknown_chain() {
    let output = cmd()
        .args([
            "swap",
            "--chain",
            "notachain",
            "--sell",
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "--buy",
            "0x4200000000000000000000000000000000000006",
            "--amount",
            "100",
            "--yes",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("invalid JSON error");
    assert_eq!(json["code"], "CHAIN_NOT_SUPPORTED");
}

#[test]
fn test_swap_no_wallet_json_error() {
    let (mut cmd, _tmp) = cmd_in_temp_home();
    // API key must be set, otherwise the failure surfaces as API_KEY_MISSING
    // before we ever reach the wallet load.
    cmd.env("ZEROX_API_KEY", "dummy-test-key");
    let output = cmd
        .args([
            "swap",
            "--chain",
            "base",
            "--sell",
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "--buy",
            "0x4200000000000000000000000000000000000006",
            "--amount",
            "1000000",
            "--yes",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("invalid JSON error");
    assert_eq!(json["code"], "WALLET_NOT_FOUND");
    assert_eq!(json["category"], "config");
    assert_eq!(json["retryable"], false);
}

#[test]
fn test_swap_no_wallet_envelope_error() {
    let (mut cmd, _tmp) = cmd_in_temp_home();
    cmd.env("ZEROX_API_KEY", "dummy-test-key");
    let output = cmd
        .args([
            "swap",
            "--chain",
            "base",
            "--sell",
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "--buy",
            "0x4200000000000000000000000000000000000006",
            "--amount",
            "100",
            "--yes",
            "-o",
            "json-envelope",
        ])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("invalid JSON envelope");

    assert_eq!(json["version"], "1");
    assert_eq!(json["status"], "error");
    assert!(json["error"]["code"].is_string());
    assert!(json["error"]["category"].is_string());
    assert!(json["error"]["retryable"].is_boolean());
}

#[test]
fn test_solana_swap_no_wallet() {
    let (mut cmd, _tmp) = cmd_in_temp_home();
    cmd.env("ZEROX_API_KEY", "dummy-test-key")
        .env_remove("ZEROX_SOLANA_KEYPAIR");
    let output = cmd
        .args([
            "swap",
            "--chain",
            "solana",
            "--sell",
            "So11111111111111111111111111111111111111112",
            "--buy",
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "--amount",
            "1000000000",
            "--yes",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("invalid JSON error");
    assert_eq!(json["code"], "WALLET_NOT_FOUND");
}

#[test]
fn test_cross_chain_no_wallet() {
    let (mut cmd, _tmp) = cmd_in_temp_home();
    cmd.env("ZEROX_API_KEY", "dummy-test-key");
    let output = cmd
        .args([
            "cross-chain",
            "--from",
            "base",
            "--to",
            "arbitrum",
            "--sell",
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "--buy",
            "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
            "--amount",
            "100",
            "--select-quote",
            "0",
            "--yes",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("invalid JSON error");
    assert_eq!(json["code"], "WALLET_NOT_FOUND");
}

// ─── Config ───────────────────────────────────────────────────

#[test]
fn test_config_path() {
    cmd()
        .args(["config", "path", "-o", "human"])
        .assert()
        .success()
        .stdout(predicate::str::contains(".0x-config"));
}

#[test]
fn test_config_set_plaintext_stores_in_file() {
    let (mut cmd, _tmp) = cmd_in_temp_home();
    let output = cmd
        .args([
            "config",
            "set",
            "wallet.evm",
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            "--plaintext",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(json["key"], "wallet.evm");
    assert_eq!(json["value"], "***redacted***");
    assert_eq!(json["storage"], "config");
}

#[test]
fn test_config_set_solana_path_stays_in_config() {
    let (mut cmd, _tmp) = cmd_in_temp_home();
    let output = cmd
        .args([
            "config",
            "set",
            "wallet.solana",
            "/tmp/some-keypair.json",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(json["storage"], "config");
}

#[test]
fn test_config_unset_clears_plaintext() {
    let (mut set_cmd, tmp) = cmd_in_temp_home();
    set_cmd
        .args([
            "config",
            "set",
            "wallet.evm",
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            "--plaintext",
            "-o",
            "json",
        ])
        .assert()
        .success();

    // Re-use the temp HOME for unset + get.
    let mut unset = Command::cargo_bin("0x").unwrap();
    unset.env("HOME", tmp.path());
    let out = unset
        .args(["config", "unset", "wallet.evm", "-o", "json"])
        .output()
        .expect("unset failed");
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(json["changed"], true);

    let mut get = Command::cargo_bin("0x").unwrap();
    get.env("HOME", tmp.path());
    let out = get
        .args(["config", "get", "wallet.evm", "-o", "json"])
        .output()
        .expect("get failed");
    assert!(!out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(json["code"], "CONFIG_NOT_FOUND");
}

#[test]
fn test_config_unset_noop_returns_changed_false() {
    let (mut cmd, _tmp) = cmd_in_temp_home();
    let output = cmd
        .args(["config", "unset", "rpc.never-set", "-o", "json"])
        .output()
        .expect("failed to run");
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(json["key"], "rpc.never-set");
    assert_eq!(json["changed"], false);
}

#[test]
fn test_config_unset_rpc_clears_it() {
    let (mut set_cmd, tmp) = cmd_in_temp_home();
    set_cmd
        .args([
            "config",
            "set",
            "rpc.base",
            "https://base.example.com",
            "-o",
            "json",
        ])
        .assert()
        .success();

    let mut unset = Command::cargo_bin("0x").unwrap();
    unset.env("HOME", tmp.path());
    let out = unset
        .args(["config", "unset", "rpc.base", "-o", "json"])
        .output()
        .expect("unset failed");
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(json["changed"], true);

    let mut show = Command::cargo_bin("0x").unwrap();
    show.env("HOME", tmp.path());
    let out = show
        .args(["config", "show", "-o", "json"])
        .output()
        .expect("show failed");
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(json["rpc"], serde_json::json!({}));
}

// ─── Telemetry ────────────────────────────────────────────────

/// The test binary is built without a compiled-in Amplitude key, so telemetry
/// must be fully inert: running a command writes no queue file and mints no
/// install id, regardless of opt-out env. This guards that dev/CI builds never
/// phone home or leave telemetry artifacts.
#[test]
fn test_telemetry_inert_without_compiled_key() {
    let (mut cmd, tmp) = cmd_in_temp_home();
    cmd.args(["chains", "-o", "json"]).assert().success();

    let queue = tmp.path().join(".0x-config/telemetry-queue.jsonl");
    assert!(!queue.exists(), "telemetry queue should not exist in a keyless build");

    // No install id should have been persisted either.
    let config = tmp.path().join(".0x-config/config.toml");
    if config.exists() {
        let body = std::fs::read_to_string(&config).unwrap();
        assert!(
            !body.contains("install_id"),
            "no install id should be minted without a compiled key"
        );
    }
}

/// Opt-out env vars are honored without error and leave no telemetry trace.
#[test]
fn test_telemetry_opt_out_env_leaves_no_trace() {
    for (k, v) in [("DO_NOT_TRACK", "1"), ("ZEROX_TELEMETRY", "0")] {
        let (mut cmd, tmp) = cmd_in_temp_home();
        cmd.env(k, v).args(["chains", "-o", "json"]).assert().success();
        let queue = tmp.path().join(".0x-config/telemetry-queue.jsonl");
        assert!(!queue.exists(), "{k}={v} should produce no telemetry queue");
    }
}

/// `telemetry.enabled` is a first-class config key end-to-end.
#[test]
fn test_telemetry_config_roundtrip() {
    let (mut set_cmd, tmp) = cmd_in_temp_home();
    set_cmd
        .args(["config", "set", "telemetry.enabled", "false", "-o", "json"])
        .assert()
        .success();

    let mut show = Command::cargo_bin("0x").unwrap();
    show.env("HOME", tmp.path());
    let out = show
        .args(["config", "show", "-o", "json"])
        .output()
        .expect("show failed");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(json["telemetry"]["enabled"], false);
}

// ─── Completions ──────────────────────────────────────────────

#[test]
fn test_completions_bash() {
    cmd()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_0x"));
}

#[test]
fn test_completions_zsh() {
    cmd()
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("#compdef"));
}

// ─── Additional Error Tests ───────────────────────────────────

#[test]
fn test_price_without_api_key_exits_5() {
    let (mut cmd, _tmp) = cmd_in_temp_home();
    let output = cmd
        .env_remove("ZEROX_API_KEY")
        .args([
            "price",
            "--chain",
            "base",
            "--sell",
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "--buy",
            "0x4200000000000000000000000000000000000006",
            "--amount",
            "1000000",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("invalid JSON error");
    assert_eq!(json["code"], "API_KEY_MISSING");
    assert_eq!(json["category"], "config");
}

#[test]
fn test_status_gasless_without_chain() {
    let output = cmd()
        .args(["status", "0xabc123", "--type", "gasless", "-o", "json"])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    // Should require --chain
}

#[test]
fn test_swap_zero_amount_rejected() {
    let (mut cmd, _tmp) = cmd_in_temp_home();
    let output = cmd
        .env(
            "ZEROX_EVM_PRIVATE_KEY",
            "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        )
        .env("ZEROX_API_KEY", "test")
        .args([
            "swap",
            "--chain",
            "solana",
            "--sell",
            "So11111111111111111111111111111111111111112",
            "--buy",
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "--amount",
            "0",
            "--yes",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("invalid JSON error");
    // Validate base-unit amount runs first; zero amount is INPUT_INVALID.
    assert_eq!(json["code"], "INPUT_INVALID");
}

#[test]
fn test_dry_run_flag_accepted() {
    // Verify --dry-run parses and doesn't change argument validation.
    // We run with no wallet so the call short-circuits at WALLET_NOT_FOUND
    // before any RPC / API call.
    let (mut cmd, _tmp) = cmd_in_temp_home();
    cmd.env("ZEROX_API_KEY", "dummy-test-key")
        .env_remove("ZEROX_EVM_PRIVATE_KEY");
    let output = cmd
        .args([
            "swap",
            "--chain",
            "base",
            "--sell",
            "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "--buy",
            "0x4200000000000000000000000000000000000006",
            "--amount",
            "100",
            "--dry-run",
            "--yes",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to run");

    assert!(!output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("invalid JSON");
    // Wallet missing — confirms --dry-run was accepted by clap and we
    // reached the wallet-load step without an arg-parse failure.
    assert_eq!(json["code"], "WALLET_NOT_FOUND");
}

#[test]
fn chains_list_includes_tron() {
    let mut cmd = assert_cmd::Command::cargo_bin("0x").unwrap();
    cmd.args(["chains", "-o", "json"]);
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"tron\""), "chains output should list tron: {stdout}");
    assert!(stdout.contains("tvm"), "chains output should show tvm chain_type: {stdout}");
}

#[test]
fn swap_rejects_tron_with_cross_chain_hint() {
    let mut cmd = assert_cmd::Command::cargo_bin("0x").unwrap();
    cmd.args([
        "swap", "--chain", "tron",
        "--sell", "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
        "--buy", "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
        "--amount", "1000000", "-o", "json-envelope",
    ]);
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("cross-chain"), "expected cross-chain hint, got: {stdout}");
}
