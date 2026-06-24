# Solana swaps

Same `price` / `swap` commands with `--chain solana`. The CLI builds, signs, and submits a versioned transaction via the 0x Solana API.

## Price

```bash
0x price --chain solana \
  --sell So11111111111111111111111111111111111111112 \
  --buy EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v \
  --amount 1000000000 \
  -o json-envelope
```

Read-only — no keypair needed, no keychain prompt.

## Swap

```bash
0x swap --chain solana \
  --sell So11111111111111111111111111111111111111112 \
  --buy EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v \
  --amount 1000000000 \
  --yes -o json-envelope
```

Requires a Solana keypair: `0x config set wallet.solana <path-or-base58>` or `ZEROEX_SOLANA_KEYPAIR`. The transaction is simulated before submission; simulation failures exit 10 with the first simulation logs in `error.details.simulation_logs`.

## Flag differences vs EVM (important)

| Flag | Behavior on Solana |
|------|--------------------|
| `--gasless` | **Rejected**, exit 2 — gasless is EVM-only. Solana fees are sub-cent and unavoidable. |
| `--recipient` | **Rejected**, exit 2 — tokens always go to the signer. Transfer separately after the swap if needed. |
| `--approval` | Ignored with a `FLAG_IGNORED` warning — Solana has no allowance model. |
| `--slippage` | Works the same (basis points). |

## Amounts

- SOL has 9 decimals: `1000000000` lamports = 1 SOL.
- Solana USDC (`EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v`) has 6 decimals.
- Mints are base58 — never mix EVM 0x addresses into a Solana swap.
