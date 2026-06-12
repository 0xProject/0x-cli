# Cross-chain swaps (bridging)

Swap a token on one chain for a token on another in a single command. Supports EVM↔EVM, EVM↔Solana. The CLI fetches multiple bridge quotes, executes the origin-chain transaction, and can track the bridge until funds land on the destination.

## Quote + execute

```bash
0x cross-chain --from base --to arbitrum \
  --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0xaf88d065e77c8cC2239327C5EDb3A432268e5831 \
  --amount 1000000 \
  --select-quote best-price \
  --yes -o json-envelope
```

- `--select-quote` accepts `best-price`, `fastest`, or a numeric index (`0`, `1`, …).
- `--sort price|speed` (default `price`) orders the fetched quotes; `--max-quotes` (default 3) caps how many are fetched.
- `--sell` is validated against the origin chain's address format, `--buy` against the destination's.

## The two-step agent pattern

Without `--yes`, the CLI emits the selected quote as a preview envelope and exits **25** — nothing is signed. Use this to show the user the bridge, rate, and ETA before committing:

```bash
# Step 1: preview (exit 25, data has bridge/buy_amount/estimated_time_seconds)
0x cross-chain --from base --to arbitrum ... --select-quote best-price -o json-envelope

# Step 2: user approved → same command + --yes
0x cross-chain --from base --to arbitrum ... --select-quote best-price --yes -o json-envelope
```

With `--dry-run`, the CLI returns **all** fetched quotes in the envelope (exit 30) without selecting or executing — useful for comparing routes.

## Solana-origin routes and the ephemeral signer

Some Solana-origin routes (Circle CCTP: `circle_forwarder_fast`, `circle_forwarder_standard`) require an extra one-shot transaction signer. The CLI handles this automatically: it generates a fresh keypair per quote request, sends its pubkey as `solanaEphemeralSignerPubkey`, and co-signs the selected transaction at submission. No flag exists and no agent action is needed — these routes simply appear in the quote list alongside the others. The keypair is in-memory only and discarded after the command.

## Pre-flight checks

- No route / no liquidity → exit 6, `NO_LIQUIDITY`.
- Insufficient sell-token balance on the origin chain (reported by the quote) → exit 6, `INSUFFICIENT_BALANCE`, before confirmation.
- Origin-chain approvals are handled automatically when the quote flags one.

## Tracking the bridge

A successful origin transaction usually exits **12** (bridge still in flight) with `data.origin_tx_hash`. Poll:

```bash
0x status <origin_tx_hash> --type cross-chain --chain base --poll -o json-envelope
```

- `--chain` is the **origin** chain.
- Exits 0 when the bridge fills (`data.successful: true`); `data.transactions[]` lists per-chain tx hashes with explorer URLs.
- Exits 11 if the bridge fails or the origin tx reverted; `data.failure_reason` explains why.
- Bridges can take minutes — prefer `--poll` over re-invoking, and surface `estimated_time_seconds` to the user up front.
