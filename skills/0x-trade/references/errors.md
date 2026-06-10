# Error catalog and recovery playbook

Every error carries: `code` (stable), `category`, `retryable`, `message`, and usually a `suggestion`. The process exit code is derived from the error code. Match on `exit_code` first, then `error.code`.

## Catalog

| `error.code` | Category | Exit | Retryable | Recovery |
|--------------|----------|-----:|:---------:|----------|
| `CONFIG_NOT_FOUND` | config | 3 | no | Run `0x config init`. |
| `CONFIG_INVALID` | config | 3 | no | Inspect `~/.0x-config/config.toml`; fix or re-init. |
| `API_KEY_MISSING` | config | 5 | no | `0x config set api_key <key>` or `ZEROX_API_KEY`. |
| `WALLET_NOT_FOUND` | config | 3 | no | `0x config set wallet.evm <key>` / `wallet.solana <path>`. |
| `WALLET_INVALID` | config | 3 | no | The stored key/keypair doesn't parse — re-set it. |
| `KEYRING_UNAVAILABLE` | config | 3 | no | No OS keyring (headless Linux) — re-set the secret with `--plaintext` or use env vars. |
| `INPUT_INVALID` | input | 2 | no | Fix the arguments (also fired for EVM-only flags on Solana). |
| `CHAIN_NOT_SUPPORTED` | input | 2 | no | `0x chains` for the supported list. |
| `INSUFFICIENT_BALANCE` | validation | 6 | no | Wallet holds less sell token than `--amount`. `error.details` has `{token, actual, expected}`. Fund or reduce. |
| `INSUFFICIENT_ALLOWANCE` | validation | 6 | no | Rare — approvals are normally automatic. Re-run; the CLI approves from the fresh quote. |
| `NO_LIQUIDITY` | validation | 6 | no | Different pair, smaller amount, or different chain. |
| `TOKEN_NOT_SUPPORTED` | validation | 6 | no | Verify the address is correct **for this chain**. |
| `SELL_AMOUNT_TOO_SMALL` | validation | 6 | no | Increase `--amount`. |
| `NETWORK_ERROR` / `NETWORK_TIMEOUT` | network | 4 | yes | Retry with backoff. |
| `RPC_ERROR` / `API_RATE_LIMITED` | network | 4 | yes | Retry with backoff; consider a private RPC (`rpc.<chain>`). |
| `INTERNAL_SERVER_ERROR` | api | 4 | yes | 0x-side 5xx — retry with backoff. |
| `API_ERROR` | api | 4 | no | Unclassified API failure — read `error.message`; include `metadata.zid` if reporting. |
| `API_ACCESS_DENIED` | api | 5 | no | Key is valid but the plan lacks this endpoint (e.g. Solana, cross-chain) — user must contact 0x. |
| `SIGNING_FAILED` / `INVALID_SIGNATURE` | signing | 1 | no | Wallet/key problem — verify the configured key. |
| `SIMULATION_FAILED` | execution | 10 | no* | *Special case below.* |
| `TRANSACTION_REVERTED` | execution | 11 | no | On-chain revert — don't retry as-is; explain to the user. |
| `TRANSACTION_TIMEOUT` | execution | 12 | yes | Tx may still land — poll `0x status` before doing anything else. |
| `QUOTE_EXPIRED` | execution | 1 | yes | Fetch a fresh quote (just re-run). |
| `BRIDGE_FAILED` | bridge | 1 | no | Check `data.failure_reason` via `0x status`. |
| `BRIDGE_TIMEOUT` | bridge | 12 | yes | Bridge in flight — keep polling `0x status --type cross-chain`. |
| `USER_CANCELLED` | input | 20 | no | The user said no. Stop. |

## SIMULATION_FAILED (exit 10) — the special case

A simulation failure is **ambiguous**: it can be a transient infrastructure issue (RPC hiccup, stale gas estimate, node state lag) or a real problem (the route would revert, balance changed since the quote). The envelope marks it `retryable: false` to stop blind retry loops, but the right behavior is:

1. Read `error.message` (and `error.details.simulation_logs` on Solana) for the revert reason.
2. Check the obvious causes: sell-token balance, slippage too tight, token addresses.
3. If nothing looks wrong, **one** retry is reasonable — transient RPC issues do happen, especially on public endpoints.
4. If it fails again, treat it as real: change parameters or surface the error to the user. Never loop.

## Pending work (exit 12) — poll, don't panic

Gasless swaps and cross-chain bridges return exit 12 when submitted but not yet terminal. The lookup key is in `data` (`trade_hash` for gasless, `origin_tx_hash` for cross-chain):

```bash
0x status <trade_hash> --type gasless --chain base --poll -o json-envelope
0x status <origin_tx_hash> --type cross-chain --chain base --poll -o json-envelope
```

Exit 0 = confirmed/filled; exit 11 = reverted/failed. Always pass `--type` — the two hash kinds are visually identical.
