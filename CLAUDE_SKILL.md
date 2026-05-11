# Claude Skill: 0x Token Trading

## Skill Definition

This skill lets Claude trade tokens across EVM chains and Solana via the `0x` CLI — a single Rust binary wrapping the 0x Swap, Gasless, Solana, and Cross-Chain APIs.

## When to trigger

- User asks to swap / trade / exchange / convert tokens.
- User asks about token prices, rates, or quotes.
- User asks to bridge tokens between chains.
- User mentions 0x Protocol, DEX aggregation, or on-chain trading.

## Capabilities

- Indicative price check (`0x price`) — read-only, no wallet needed.
- EVM swap via 0x Allowance Holder flow (auto-approves the spender when allowance is short).
- EVM gasless swap (`--gasless`) — no ETH/gas needed; 0x submits the trade on the user's behalf.
- Solana swap — builds, signs, and submits a versioned transaction.
- Cross-chain swap (`0x cross-chain`) with bridge-quote selection (`--select-quote best-price | fastest | <index>`).
- Transaction / bridge status polling (`0x status ... --poll`, configurable `--poll-interval`).
- Chain discovery (`0x chains`), config inspection (`0x config show` / `0x config get`).
- Config writes with OS-keyring storage for wallet secrets, plus `0x config unset` to remove them.

## Prerequisites

Before using the CLI:

```bash
0x config init                # one-time interactive setup (API key, default chain, wallet)
0x config show -o json-envelope
0x chains -o json-envelope     # confirm the chain you want is supported
```

Required: a 0x API key. EVM swaps need an EVM private key; Solana swaps need a Solana keypair (file path or base58). Price checks need only the API key.

**Wallet secrets are stored in the OS keyring** (macOS keychain, Linux libsecret, Windows credential locker) by default. `0x config show` reads back `<stored in keyring>` for wallet fields that live there. To store secrets in the config file instead (e.g. headless Linux without a keyring daemon), pass `--plaintext`:

```bash
0x config set wallet.evm 0xac0974...                  # default: keyring
0x config set wallet.evm 0xac0974... --plaintext      # opt-out: config file
0x config set wallet.solana /path/to/keypair.json     # file paths always stay in config (path isn't secret)
```

Env vars (`ZEROX_EVM_PRIVATE_KEY`, `ZEROX_SOLANA_KEYPAIR`) and `--wallet <value>` take precedence over both the keyring and the config file.

## Output contract (agents read this)

The CLI auto-detects non-TTY stdout and emits a JSON envelope. **Pass `-o json-envelope` explicitly** when invoking from an agent for stability; never rely on auto-detection inside a tool harness.

**Envelope on success:**
```json
{
  "version": "1",
  "command": "price",
  "timestamp": "2026-05-11T17:38:35Z",
  "duration_ms": 423,
  "exit_code": 0,
  "status": "success",
  "data": { /* command-specific payload, see examples below */ },
  "warnings": [],
  "metadata": { "chain_id": 8453, "chain_name": "Base", "zid": "0x-trace-id" }
}
```

**Envelope on error:**
```json
{
  "version": "1",
  "command": "swap",
  "timestamp": "...",
  "duration_ms": 120,
  "exit_code": 5,
  "status": "error",
  "error": {
    "code": "API_KEY_MISSING",
    "message": "No API key configured",
    "category": "config",
    "retryable": false,
    "suggestion": "Run '0x config set api_key <your-key>' or set ZEROX_API_KEY env var"
  },
  "metadata": { ... }
}
```

Match on `exit_code` first (stable), then `error.code` (also stable). `error.retryable` tells you whether a retry makes sense.

## Common commands

### Price check (read-only, EVM)
```bash
0x price --chain base \
  --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0x4200000000000000000000000000000000000006 \
  --amount 1000000 \
  -o json-envelope
```
`data` shape: `{ chain, sell_token, buy_token, sell_amount: {raw, formatted, usd_value?}, buy_amount, min_buy_amount, rate, gas_estimate?, route[], liquidity_available }`.

### Price check (Solana)
```bash
0x price --chain solana \
  --sell So11111111111111111111111111111111111111112 \
  --buy EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v \
  --amount 1000000000 \
  -o json-envelope
```

### EVM swap (interactive prompt; use `--yes` to skip)
```bash
0x swap --chain base \
  --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0x4200000000000000000000000000000000000006 \
  --amount 1000000 \
  --slippage 100 \
  --yes -o json-envelope
```
`data` shape on success: `{ chain, sell_token, buy_token, sell_amount, buy_amount, min_buy_amount, rate, gas_used, effective_gas_price, route[], tx_hash, explorer_url, block_number, dry_run: false }`.

### Gasless EVM swap (no ETH/gas required)
```bash
0x swap --chain base \
  --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0x4200000000000000000000000000000000000006 \
  --amount 1000000 \
  --gasless --yes -o json-envelope
```
`data` includes `trade_hash` (for status lookup) in addition to `tx_hash` once the trade lands.

### Solana swap
```bash
0x swap --chain solana \
  --sell So11111111111111111111111111111111111111112 \
  --buy EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v \
  --amount 1000000000 \
  --yes -o json-envelope
```
`--gasless`, `--recipient`, `--approval` are EVM-only and silently ignored on Solana.

### Cross-chain swap
```bash
0x cross-chain --from base --to arbitrum \
  --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0xaf88d065e77c8cC2239327C5EDb3A432268e5831 \
  --amount 1000000 \
  --select-quote best-price \
  --yes -o json-envelope
```
`--select-quote` accepts `best-price`, `fastest`, or a numeric index (`0`, `1`, ...). Without `--yes`, the CLI fetches quotes and exits 20 with a quote preview so the agent can pick.

### Status polling

The hash type is auto-detected (gasless trades use a custom hash format; cross-chain origin txs are 0x-prefixed 66-char hex). Pass `--type` for reliability.

```bash
# Gasless trade status
0x status <trade_hash> --type gasless --chain base --poll -o json-envelope

# Cross-chain bridge status
0x status <origin_tx_hash> --type cross-chain --chain base --poll -o json-envelope
```

## Error handling pattern

When a long-running command returns exit 12 (pending / timed out, but the tx is on-chain), poll status until terminal. Exit 12 happens with **gasless** and **cross-chain** swaps — pick the matching `--type`:

```bash
# Gasless swap pending → poll with the trade hash
$ 0x swap --chain base --sell USDC --buy WETH --amount 1000000 --gasless --yes -o json-envelope
# → exit 12, data.trade_hash is the lookup key
$ 0x status <trade_hash> --type gasless --chain base --poll -o json-envelope
# → exits 0 when confirmed, or 11 if failed

# Cross-chain pending → poll with the origin tx hash
$ 0x cross-chain --from base --to arbitrum ... --yes -o json-envelope
# → exit 12, data.origin_tx_hash is the lookup key
$ 0x status <origin_tx_hash> --type cross-chain --chain base --poll -o json-envelope
# → exits 0 when bridge_filled, or 11 if reverted
```

Always inspect `error.retryable` before retrying. `retryable: false` codes (`SIMULATION_FAILED`, `INSUFFICIENT_BALANCE`, `TRANSACTION_REVERTED`) should not be retried with the same parameters.

## Exit Code Decision Tree

| Code | Meaning | Agent action |
|------|---------|--------------|
| 0 | Success | Report result. |
| 1 | General error | Inspect `error.code` and `error.message`. |
| 2 | Validation error (bad input, insufficient balance, no liquidity) | Fix parameters. Do not retry blindly. |
| 3 | Config error (missing/invalid config or wallet) | Guide user through `0x config init` or env var setup. |
| 4 | Network error (rate limit, timeout, 5xx) | Retry with backoff. |
| 5 | Auth error — API key missing or rejected | Ask user to update key. |
| 10 | Simulation failed | Do NOT retry same params. Surface the simulation error. |
| 11 | Transaction reverted on-chain | Do NOT retry same params. Explain to user. |
| 12 | Pending / timeout, but tx may be on chain | Poll with `0x status`. |
| 20 | User confirmation required (only when `--yes` not passed) | Pass `--yes` for non-interactive, or show the quote to the user. |
| 30 | Dry-run completed | Informational; report simulated result. |

## Important notes

1. **Amounts are in base units.** USDC has 6 decimals: `--amount 1000000` = 1 USDC. ETH has 18: `--amount 1000000000000000000` = 1 ETH. Solana SOL has 9: `--amount 1000000000` = 1 SOL. The envelope's `data.*.formatted` shows the human-readable form.

2. **Always dry-run first for material amounts.** `--dry-run` is a global flag. Exit code 30, no on-chain tx:
   ```bash
   0x swap --chain base --sell USDC --buy WETH --amount 1000000 --yes --dry-run -o json-envelope
   ```

3. **Token addresses are chain-specific.** USDC on Base is `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`; on Ethereum it's `0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48`. Don't reuse addresses across chains.

4. **EVM swap = Allowance Holder.** The CLI inspects the quote's `issues.allowance` and sends an ERC-20 `approve` to the indicated spender automatically (using `--approval exact` by default; `--approval unlimited` for max approval).

5. **EVM-only flags.** `--gasless`, `--recipient`, `--approval` are EVM-only — ignored on Solana.

6. **`--yes` skips the confirmation prompt** but is meaningless for `price` (read-only) and `chains` / `config *` (no prompt). Pass it for `swap`, `cross-chain`, and any status-polling that might prompt.

7. **`zid` in metadata** is the 0x trace ID — include it in support requests when something looks wrong.

## Supported chains

| ID | Name | Network | Native |
|---:|------|---------|--------|
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
| 34443 | mode | Mode | ETH |
| 42161 | arbitrum | Arbitrum | ETH |
| 43114 | avalanche | Avalanche | AVAX |
| 57073 | ink | Ink | ETH |
| 59144 | linea | Linea | ETH |
| 80094 | berachain | Berachain | BERA |
| 81457 | blast | Blast | ETH |
| 534352 | scroll | Scroll | ETH |
| solana | solana | Solana | SOL |

Use `0x chains -o json-envelope` for the live list (with explorer URLs and chain types).

## Common token reference

| Chain | Token | Address | Decimals |
|-------|-------|---------|---------:|
| Base | USDC | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | 6 |
| Base | WETH | `0x4200000000000000000000000000000000000006` | 18 |
| Base | cbBTC | `0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf` | 8 |
| Ethereum | USDC | `0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48` | 6 |
| Ethereum | WETH | `0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2` | 18 |
| Arbitrum | USDC | `0xaf88d065e77c8cC2239327C5EDb3A432268e5831` | 6 |
| Arbitrum | WETH | `0x82aF49447D8a07e3bd95BD0d56f35241523fBab1` | 18 |
| Solana | SOL (wrapped) | `So11111111111111111111111111111111111111112` | 9 |
| Solana | USDC | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` | 6 |

Always verify addresses on a block explorer before sending material value.
