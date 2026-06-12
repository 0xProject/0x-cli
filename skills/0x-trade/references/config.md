# Config, wallets, and environment

Non-secret config lives in `~/.0x-config/config.toml`. Wallet secrets go to the **OS keyring** by default (macOS Keychain, Linux libsecret, Windows Credential Locker).

## First-time setup

```bash
0x config init            # interactive wizard: API key, default chain, wallet
0x config init --browser  # also opens dashboard.0x.org to grab an API key
```

Non-interactive (agent-driven) setup:

```bash
0x config set api_key <key>
0x config set defaults.chain base
0x config set wallet.evm 0xac0974...                # secret → OS keyring
0x config set wallet.solana /path/to/keypair.json   # path → config file (paths aren't secret)
```

## Keys

| Key | Meaning |
|-----|---------|
| `api_key` | 0x API key (config file) |
| `defaults.chain` | Default chain when `--chain` is omitted |
| `defaults.slippage_bps` | Default slippage in basis points |
| `defaults.approval_type` | `exact` or `unlimited` |
| `rpc.<chain>` | Custom RPC URL per chain, e.g. `rpc.base` |
| `wallet.evm` | EVM private key (hex) — secret → keyring |
| `wallet.solana` | Keypair file path (→ config) or base58/JSON-array secret (→ keyring) |

- `--plaintext` on `config set` stores a wallet secret in the config file instead of the keyring (for headless Linux without a keyring daemon).
- `0x config show` redacts secrets; keyring entries read back as `<stored in keyring>`.
- `0x config unset wallet.evm` removes both the config entry and the keyring entry.
- `0x config get <key>` reads one value; `0x config path` prints the config directory.

## Environment variables (override config)

| Var | Overrides |
|-----|-----------|
| `ZEROX_API_KEY` | `api_key` |
| `ZEROX_EVM_PRIVATE_KEY` | `wallet.evm` |
| `ZEROX_SOLANA_KEYPAIR` | `wallet.solana` (path or base58) |
| `ZEROX_DEFAULT_CHAIN` | `defaults.chain` |
| `ZEROX_RPC_URL` | RPC for the current command |
| `ZEROX_OUTPUT` | `-o/--output` format |
| `NO_COLOR` | Disables colored output |

Precedence everywhere: CLI flag > environment variable > config file > built-in default.

## RPC notes

Each chain ships a built-in public RPC fallback. Public endpoints throttle; if a swap fails with rate-limit or timeout errors, the error suggestion will say so — configure a private RPC with `0x config set rpc.<chain> <url>` or pass `--rpc-url`.

## Profiles

Named API environments stored in the config file. Each profile may override
`base_url`, `api_key`, or both; unset fields fall back to the default `[api]`
section.

```bash
0x config set profiles.stg.base_url <staging-url>
0x config set profiles.stg.api_key <staging-key>
0x config use stg          # sticky: all commands use the profile
0x --profile stg price ... # one-off
0x config use default      # back to production
```

When a profile is active, every command prints `Profile '<name>' → <url>` on
stderr. `ZEROX_PROFILE` selects a profile per-environment; `--api-key` /
`ZEROX_API_KEY` still beat the profile's key.
