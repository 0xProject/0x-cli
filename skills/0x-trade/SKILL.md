---
name: 0x-trade
description: Trade tokens with the `0x` CLI across 20+ EVM chains and Solana, powered by the 0x Swap, Gasless, Solana, and Cross-Chain APIs. Use when the user wants to swap, trade, exchange, or convert tokens; bridge tokens between chains; check token prices, rates, or quotes; run gasless swaps; or mentions 0x Protocol, DEX aggregation, or on-chain trading. Detailed guides for gasless, cross-chain, Solana, config, tokens, and error handling load on demand from references/.
allowed-tools: Bash, Read
---

# 0x Trading CLI

A single Rust binary, `0x`, that wraps the 0x APIs: indicative prices, on-chain swaps, gasless swaps, cross-chain bridging, and status polling. Every command is non-interactive-safe and emits a machine-readable JSON envelope — built to be driven by an agent.

## The agent contract

1. **Always pass `--yes -o json-envelope`** on anything that executes (`swap`, `cross-chain`). Auto-detection picks JSON for non-TTY stdout, but don't rely on it inside a tool harness.
2. **Amounts are base units** — the token's smallest unit, no decimals applied. A 6-decimal token: `1000000` = 1.0; an 18-decimal token: `1000000000000000000` = 1.0; a 9-decimal token (Solana): `1000000000` = 1.0. The envelope's `data.*.formatted` carries the human-readable form.
3. **Match on `exit_code` first** (stable), then `error.code` (also stable). `error.retryable` says whether a retry can help; `error.suggestion` says what to do instead.
4. **Dry-run material amounts first.** `--dry-run` is a global flag — simulates everything, signs/submits nothing, exits 30.

## Setup check

Run once before trading:

```bash
0x config show -o json-envelope    # API key + wallet present?
0x chains -o json-envelope         # is the target chain supported?
```

A 0x API key is always required. EVM swaps need an EVM private key, Solana swaps a Solana keypair; price checks need only the API key. If anything is missing, see `references/config.md`.

## Core flows

### Price check (read-only, no wallet)

```bash
0x price --chain base \
  --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0x4200000000000000000000000000000000000006 \
  --amount 1000000 \
  -o json-envelope
```

`data`: `{ chain, sell_token, buy_token, sell_amount: {raw, formatted, usd_value?}, buy_amount, min_buy_amount, rate, gas_estimate?, route[], liquidity_available }`

### Swap (single chain)

```bash
0x swap --chain base \
  --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0x4200000000000000000000000000000000000006 \
  --amount 1000000 \
  --slippage 100 \
  --yes -o json-envelope
```

- `--slippage` is basis points (100 = 1%).
- Approvals are automatic: the CLI reads the quote's allowance issue and sends the ERC-20 `approve` itself (`--approval exact` by default, `unlimited` opt-in). Never handle approvals manually.
- Insufficient sell-token balance is caught pre-flight from the quote and exits 6 with `INSUFFICIENT_BALANCE` — fund the wallet or lower `--amount`; don't retry as-is.
- `data` on success adds: `tx_hash, explorer_url, block_number, gas_used, effective_gas_price, dry_run`.

## Output envelope

```json
{
  "version": "1",
  "command": "swap",
  "timestamp": "2026-06-10T17:38:35Z",
  "duration_ms": 423,
  "exit_code": 0,
  "status": "success",
  "data": { },
  "warnings": [],
  "metadata": { "chain_id": 8453, "chain_name": "Base", "zid": "0x-trace-id" }
}
```

On error, `status` is `"error"` and a structured `error` object replaces `data`:

```json
"error": {
  "code": "INSUFFICIENT_BALANCE",
  "message": "Insufficient sell token balance: wallet holds 0 but the swap needs 1000000 (token 0x8335...)",
  "category": "validation",
  "retryable": false,
  "suggestion": "Fund the wallet with more of the sell token or reduce --amount"
}
```

`metadata.zid` is the 0x trace ID — include it when reporting problems to 0x support.

## Exit code decision tree

| Code | Meaning | Agent action |
|-----:|---------|--------------|
| 0 | Success | Report the result. |
| 1 | General error | Inspect `error.code` and `error.message`. |
| 2 | Input error (malformed args, unsupported chain, EVM-only flag on Solana) | Fix the command; don't retry as-is. |
| 3 | Config error (missing/invalid config or wallet) | Guide the user through setup — `references/config.md`. |
| 4 | Network error (rate limit, timeout, 5xx) | Retry with backoff. |
| 5 | Auth error (API key missing/rejected, plan lacks the endpoint) | Ask the user to check the key. |
| 6 | Validation failed (no liquidity, insufficient sell-token balance, token not supported) | Fix the parameters or fund the wallet; don't retry as-is. |
| 10 | Simulation failed | May be transient (RPC hiccup) or real (revert path). Check `error.message` and balances; one retry is reasonable, never loop. |
| 11 | Transaction reverted on-chain | Don't retry as-is; explain to the user. |
| 12 | Pending / timed out — tx may still land | Poll with `0x status` (see references). |
| 20 | User declined the confirmation prompt | Stop; don't re-run. |
| 40 | Agent-payment challenge invalid (`--pay`) — no payable scheme offered | Nothing spent; report it. |
| 41 | Payment exceeded `--max-payment` — refused before signing | **Nothing spent.** Only raise the cap if the amount is expected. |
| 42 | Payment signing failed | Verify the payment wallet; nothing spent. |
| 43 | Payment settlement failed | **Money may have been spent** with no usable result — don't blindly retry; check the wallet. |
| 44 | Payment wallet unfunded (USDC/USDC.e or native gas) | Fund the payment wallet. |
| 25 | Preview emitted, confirmation required (`--yes` missing) | Show the quote to the user, or re-run with `--yes`. |
| 30 | Dry-run completed | Report the simulated result. |

The full error-code catalog (code → category → retryable → action) is in `references/errors.md`.

## Paying per request instead of an API key (`--pay`)

`price` and `swap` accept `--pay <x402-evm|mpp>` to pay ~$0.01 in USDC per request through the 0x agent gateway instead of using an API key — useful for an autonomous agent with a funded wallet and no key.

- **EVM AllowanceHolder only.** Rejected (`INPUT_INVALID`) with `--gasless`, Solana, or Tron.
- **`x402-evm`**: signs an EIP-3009 USDC authorization on Base; needs USDC in the EVM wallet.
- **`mpp`**: broadcasts a USDC.e transfer on Tempo (chainId 4217); needs USDC.e **and** native gas there. Override the RPC with `--tempo-rpc` / `ZEROEX_TEMPO_RPC_URL`.
- **`--max-payment <USD>`** (default `0.05`) caps the spend; the CLI refuses *before* signing/broadcasting if the gateway asks for more (`PAYMENT_EXCEEDS_LIMIT`, exit 41 — nothing spent).
- **Every paid request costs real, non-refundable money**, including when the on-chain swap later reverts. Don't poll `price --pay` in a loop. The settlement (tx hash, payer, amount) is in `metadata.payment`.
- `swap --pay` pays only for the *quote*; the on-chain swap still uses your wallet + RPC as usual.

See the `PAYMENT_*` rows in `references/errors.md` (exit 40–44) for recovery — exit 41 means nothing was spent; exit 43 means money may have moved without a usable result.

## Going deeper — read on demand

Read the matching reference only when the task needs it:

| Task | Read |
|------|------|
| Gasless swaps (no ETH for gas), trade-hash polling | `references/gasless.md` |
| Cross-chain swaps / bridging, quote selection, bridge status | `references/cross-chain.md` |
| Solana swaps and their flag differences | `references/solana.md` |
| Config, wallets, keyring, env vars, RPC overrides | `references/config.md` |
| Supported chains and common token addresses/decimals | `references/tokens.md` |
| Full error-code catalog and recovery playbook | `references/errors.md` |

## Safety rules

- Token addresses are chain-specific — never reuse an address across chains; verify on a block explorer before moving material value.
- Prefer `--approval exact` (default) unless the user explicitly asks for unlimited.
- `--dry-run` before any swap the user would care about losing.
- Never echo private keys; wallet secrets live in the OS keyring (see `references/config.md`).
