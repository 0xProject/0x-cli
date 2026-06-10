# 0x CLI

Trade tokens across Solana and EVM chains from your terminal. Built for both human traders and AI agents.

## Quick Start

```bash
# Build from source
cargo install --path .

# Configure
0x config init

# Check a price
0x price --chain base --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0x4200000000000000000000000000000000000006 --amount 1000000

# Execute a swap
0x swap --chain base --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0x4200000000000000000000000000000000000006 --amount 1000000
```

## Features

- **4 APIs**: EVM Swap (Allowance Holder), Gasless Swap, Solana Swap, Cross-Chain
- **20 chains**: Ethereum, Base, Arbitrum, Optimism, Polygon, BSC, Avalanche, Linea, Scroll, Blast, Mantle, Berachain, Sonic, Unichain, World Chain, Abstract, Ink, Monad, HyperEVM, Solana
- **Agent-first**: Auto-detect non-TTY for JSON output, structured error codes, stable exit codes, inline `RESPONSE:` schemas in every `--help`
- **Safe by default**: OS keyring for wallet secrets, transaction simulation before every execution, `--dry-run` mode, exact token approvals
- **Rich UX**: Colored tables, progress spinners, interactive confirmation, shell completions

## Installation

```bash
# From source (requires Rust 1.75+)
git clone <repo> && cd 0x-cli
cargo install --path .

# Verify
0x --version
```

## Configuration

### Interactive Setup

```bash
0x config init
```

Guides you through setting your API key, default chain, and wallet.

### Manual Setup

```bash
# Set your 0x API key (get one at https://dashboard.0x.org)
0x config set api_key <your-key>

# Set default chain
0x config set defaults.chain base

# Set EVM wallet (private key) — goes to OS keyring by default
0x config set wallet.evm 0xac0974bec...

# Same, but force plaintext storage in the config file
0x config set wallet.evm 0xac0974bec... --plaintext

# Solana wallet: file paths stay in the config file (path isn't secret)
0x config set wallet.solana /path/to/keypair.json

# Solana wallet: base58 secrets go to the keyring
0x config set wallet.solana 4Nd1mBQt...

# Set custom RPC endpoints
0x config set rpc.base https://base.llamarpc.com
0x config set rpc.solana https://api.mainnet-beta.solana.com

# Inspect a single key (secrets redacted)
0x config get wallet.evm

# Remove a key (clears keyring + config file for wallet keys)
0x config unset wallet.evm
```

### Wallet Storage

By default, `wallet.evm` and `wallet.solana` (when given key material rather than a file path) are stored in the OS keyring — macOS Keychain, Linux libsecret/`secret-tool`, or Windows Credential Locker. They are never written to disk in plaintext.

| Scenario | Storage |
|----------|---------|
| `0x config set wallet.evm <key>` | OS keyring |
| `0x config set wallet.evm <key> --plaintext` | `~/.0x-config/config.toml` |
| `0x config set wallet.solana /path/to/file.json` | `~/.0x-config/config.toml` (it's a path) |
| `0x config set wallet.solana <base58>` | OS keyring |
| `ZEROX_EVM_PRIVATE_KEY` / `ZEROX_SOLANA_KEYPAIR` env var | Read directly, never persisted |

`0x config show` reports keyring-stored wallets as `<stored in keyring>`. If the OS keyring is unavailable (e.g. headless Linux with no DBus), use `--plaintext` or the env vars.

### Environment Variables

Environment variables always take precedence over config file values.

| Variable | Description |
|----------|-------------|
| `ZEROX_API_KEY` | 0x API key |
| `ZEROX_EVM_PRIVATE_KEY` | EVM private key (hex) |
| `ZEROX_SOLANA_KEYPAIR` | Solana keypair file path or base58 |
| `ZEROX_DEFAULT_CHAIN` | Default chain name or ID |
| `ZEROX_RPC_URL` | Override RPC URL for any chain |
| `NO_COLOR` | Disable colored output |

### Config File

Stored at `~/.0x-config/config.toml` with `0600` permissions (Unix). Wallet secrets live in the OS keyring by default — the `[wallet]` section here only contains keys you opted into plaintext for, or Solana file paths.

```toml
[api]
api_key = "your-api-key"

[defaults]
chain = "base"
slippage_bps = 100
approval_type = "exact"

[rpc]
base = "https://base.llamarpc.com"
ethereum = "https://eth.llamarpc.com"
solana = "https://api.mainnet-beta.solana.com"

[wallet]
# evm = "0xac0974bec..."       # only present when --plaintext was used
solana = "/path/to/keypair.json"  # file paths stay here (not secret)
```

## Usage

### Price Check

```bash
# EVM price (read-only, no wallet needed)
0x price --chain base \
  --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0x4200000000000000000000000000000000000006 \
  --amount 1000000

# Gasless price
0x price --chain base --sell USDC --buy WETH --amount 1000000 --gasless

# JSON output for scripting
0x price --chain base --sell USDC --buy WETH --amount 1000000 -o json
```

### EVM Swap

```bash
# Interactive swap with confirmation prompt
0x swap --chain base \
  --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0x4200000000000000000000000000000000000006 \
  --amount 1000000

# Non-interactive (for agents/scripts)
0x swap --chain base --sell USDC --buy WETH --amount 1000000 --yes -o json

# Dry run (simulate without executing)
0x swap --chain base --sell USDC --buy WETH --amount 1000000 --dry-run

# Custom slippage (200 bps = 2%)
0x swap --chain base --sell USDC --buy WETH --amount 1000000 --slippage 200

# Unlimited token approval (instead of exact amount)
0x swap --chain base --sell USDC --buy WETH --amount 1000000 --approval unlimited
```

### Gasless Swap

No gas fees required. The 0x protocol handles gas on your behalf.

```bash
0x swap --chain base --sell USDC --buy WETH --amount 1000000 --gasless
```

### Solana Swap

```bash
0x swap --chain solana \
  --sell So11111111111111111111111111111111111111112 \
  --buy EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v \
  --amount 1000000000
```

### Cross-Chain Swap

```bash
# Interactive (shows quote table, lets you pick)
0x cross-chain \
  --from base --to arbitrum \
  --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0xaf88d065e77c8cC2239327C5EDb3A432268e5831 \
  --amount 1000000

# Auto-select best price quote
0x cross-chain --from base --to arbitrum \
  --sell USDC --buy USDC --amount 1000000 \
  --select-quote best-price --yes

# Sort by fastest bridge
0x cross-chain --from base --to arbitrum \
  --sell USDC --buy USDC --amount 1000000 --sort speed
```

### Status Tracking

```bash
# Check gasless trade status
0x status 0xabc123... --type gasless --chain base

# Poll cross-chain bridge status until complete
0x status 0xdef456... --type cross-chain --chain base --poll

# Custom poll interval (10 seconds)
0x status 0xdef456... --type cross-chain --chain base --poll --poll-interval 10
```

## AI Agent Integration

The CLI is designed as a first-class tool for AI agents and scripts.

### Auto-Detection

When stdout is not a TTY (piped or redirected), output automatically switches to `json-envelope` format. Agents calling from a tool harness should still pass `-o json-envelope` explicitly for stability — don't rely on TTY detection inside an agent loop.

```bash
# These produce identical JSON output:
0x chains -o json-envelope
0x chains | cat                # auto-detects non-TTY
```

### Inline Schemas

Every command's `--help` ends with a `RESPONSE:` block documenting the `data` payload it returns. Run `0x swap --help`, `0x cross-chain --help`, etc. to see field names, types, and which fields are optional. Agents can read this without invoking the command.

### Bundled Agent Skill

The CLI bundles an agent skill (compiled into the binary, always in sync with the running version): one `SKILL.md` entry point plus deep-dive references (gasless, cross-chain, solana, config, tokens, errors) that agents read on demand.

```bash
# Install SKILL.md + references/ into ./.claude/skills/0x-trade/
0x skill install

# Install into a custom skills directory (e.g. user-level)
0x skill install --dir ~/.claude/skills

# Print the main skill to stdout
0x skill print

# Print one reference topic
0x skill print --topic errors
```

The skill explains exit codes, output envelope shape, dry-run patterns, and per-chain token references. `-o`/`--output` is ignored for `skill print` — output is always raw markdown. The canonical source lives in `skills/0x-trade/` in this repo.

### JSON Envelope

Every command produces a consistent envelope:

```json
{
  "version": "1",
  "command": "price",
  "timestamp": "2026-03-22T14:30:00.000Z",
  "duration_ms": 423,
  "exit_code": 0,
  "status": "success",
  "data": { ... },
  "warnings": [],
  "metadata": {
    "chain_id": 8453,
    "chain_name": "Base",
    "api_version": "v2"
  }
}
```

On error:

```json
{
  "version": "1",
  "command": "swap",
  "status": "error",
  "exit_code": 5,
  "error": {
    "code": "API_KEY_MISSING",
    "message": "No API key configured",
    "category": "config",
    "retryable": false,
    "suggestion": "Run '0x config set api_key <your-key>' or set ZEROX_API_KEY env var"
  }
}
```

### Exit Codes

| Code | Meaning | Agent Action |
|------|---------|-------------|
| 0 | Success | Proceed |
| 1 | General error | Inspect `error.code` |
| 2 | Input error (malformed args, unsupported chain) | Fix the command |
| 3 | Config error | Run `0x config init` |
| 4 | Network error | Retry with backoff |
| 5 | Auth error | Update API key |
| 6 | Validation failed (no liquidity, insufficient balance, token not supported) | Fix parameters or fund wallet |
| 10 | Simulation failed | Inspect the error — may be transient (RPC) or real (revert); one retry ok, never loop |
| 11 | Transaction reverted | Do NOT retry as-is |
| 12 | Transaction pending | Poll with `0x status` |
| 20 | User cancelled | Stop; don't re-run |
| 25 | Preview emitted, confirmation required | Re-run with `--yes` or show the quote |
| 30 | Dry-run completed | Informational |

### Error Codes

Each error includes a stable `code` string, `category`, and `retryable` boolean:

| Code | Category | Retryable |
|------|----------|-----------|
| `CONFIG_NOT_FOUND` | config | no |
| `API_KEY_MISSING` | config | no |
| `WALLET_NOT_FOUND` | config | no |
| `KEYRING_UNAVAILABLE` | config | no |
| `CHAIN_NOT_SUPPORTED` | input | no |
| `INSUFFICIENT_BALANCE` | validation | no |
| `NO_LIQUIDITY` | validation | no |
| `API_RATE_LIMITED` | network | yes |
| `NETWORK_TIMEOUT` | network | yes |
| `SIMULATION_FAILED` | execution | no |
| `TRANSACTION_REVERTED` | execution | no |
| `BRIDGE_FAILED` | bridge | no |
| `USER_CANCELLED` | input | no |

### Non-Interactive Flags

Every interactive prompt has a flag equivalent:

| Prompt | Flag |
|--------|------|
| Confirm trade | `--yes` / `-y` |
| Select cross-chain quote | `--select-quote <n\|best-price\|fastest>` |
| Approval strategy | `--approval exact\|unlimited` |
| Suppress progress | `--quiet` / `-q` |

### Stdout/Stderr Contract

- **stdout**: Only machine-parseable output (tables in human mode, JSON in json modes)
- **stderr**: Progress spinners, status messages, debug logs — suppressed with `--quiet`

## Global Flags

| Flag | Short | Description | Default |
|------|-------|-------------|---------|
| `--output` | `-o` | `human`, `json`, `json-envelope` | Auto-detect |
| `--yes` | `-y` | Skip confirmation prompts | false |
| `--quiet` | `-q` | Suppress stderr output | false |
| `--verbose` | `-v` | Debug output on stderr | false |
| `--dry-run` | | Simulate without executing | false |
| `--api-key` | | Override API key | From config |
| `--rpc-url` | | Override RPC URL | From config |
| `--wallet` | `-w` | Override wallet | From config |
| `--timeout` | | HTTP timeout (seconds) | 30 |
| `--no-color` | | Disable colors | Auto-detect |

## Supported Chains

| ID | Name | Network | Native Token |
|----|------|---------|-------------|
| 1 | ethereum | Ethereum | ETH |
| 10 | optimism | Optimism | ETH |
| 56 | bsc | BNB Chain | BNB |
| 130 | unichain | Unichain | ETH |
| 137 | polygon | Polygon | POL |
| 143 | monad | Monad | MON |
| 146 | sonic | Sonic | S |
| 480 | worldchain | World Chain | ETH |
| 999 | hyperevm | HyperEVM | HYPE |
| 2741 | abstract | Abstract | ETH |
| 5000 | mantle | Mantle | MNT |
| 8453 | base | Base | ETH |
| 42161 | arbitrum | Arbitrum | ETH |
| 43114 | avalanche | Avalanche | AVAX |
| 57073 | ink | Ink | ETH |
| 59144 | linea | Linea | ETH |
| 80094 | berachain | Berachain | BERA |
| 81457 | blast | Blast | ETH |
| 534352 | scroll | Scroll | ETH |
| solana | solana | Solana | SOL |

## Security

- **OS keyring by default**: Wallet secrets (`wallet.evm`, `wallet.solana` key material) are stored in the OS keyring — macOS Keychain, Linux libsecret, Windows Credential Locker. Use `--plaintext` to opt out only when the keyring isn't available.
- **Config file**: Created with `0600` permissions (owner read/write only)
- **Config directory**: Created with `0700` permissions
- **Redaction**: `0x config show` and `0x config get` never reveal secret material. Wallets stored in the keyring show as `<stored in keyring>`; plaintext wallets show as `***redacted***`; Solana file paths show verbatim because the path itself isn't sensitive.
- **Transaction simulation**: Every transaction is simulated via `eth_call` (EVM) or `simulate_transaction` (Solana) before submission
- **Approval strategy**: Default is `exact` (only approve the needed amount). Use `--approval unlimited` for max approval.
- **Environment variables**: Sensitive values like private keys can be set via env vars (`ZEROX_EVM_PRIVATE_KEY`, `ZEROX_SOLANA_KEYPAIR`) to avoid persisting them at all — read-once, never written to disk or keyring.

## Development

```bash
# Build
cargo build

# Run tests
cargo test

# Lint
cargo clippy

# Run directly
cargo run -- --help
cargo run -- chains -o human
```

### Project Structure

```
src/
  main.rs              # Entry point, GlobalOpts, command dispatch
  cli.rs               # clap derive definitions
  error.rs             # Error types, codes, exit codes
  confirm.rs           # Trade confirmation prompt
  config/              # ~/.0x-config management
  api/                 # 0x API clients (evm_swap, gasless, solana, cross_chain)
  wallet/              # Key loading (EVM PrivateKeySigner, Solana Keypair)
  chain/               # Chain operations (EVM provider + ERC-20, Solana tx building)
  output/              # Output formatting (human tables, JSON envelope)
  commands/            # Command implementations
tests/
  cli_output.rs        # Integration tests for CLI output
```
