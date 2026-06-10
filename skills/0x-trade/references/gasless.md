# Gasless swaps

EVM-only. The user's wallet needs zero native token: the CLI signs EIP-712 messages (approval + trade) locally and 0x relays the transaction. Useful when the wallet holds ERC-20s but no ETH for gas.

## Price (indicative)

```bash
0x price --chain base \
  --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0x4200000000000000000000000000000000000006 \
  --amount 1000000 \
  --gasless -o json-envelope
```

## Swap

```bash
0x swap --chain base \
  --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \
  --buy 0x4200000000000000000000000000000000000006 \
  --amount 1000000 \
  --gasless --yes -o json-envelope
```

What happens under the hood:

1. Quote fetched; balance shortfalls exit 6 (`INSUFFICIENT_BALANCE`) before anything is signed.
2. The CLI validates the EIP-712 payloads (domain, spender, amounts) before signing — a malformed or mismatched payload is refused with a structured error rather than signed blindly.
3. Trade is submitted to the 0x relayer; `data.trade_hash` is the lookup key for status.
4. The CLI polls briefly; if the trade hasn't confirmed in time it exits **12** with `trade_hash` still in `data`.

## Polling a pending trade

Exit 12 means "submitted, not yet terminal" — not failure. Poll:

```bash
0x status <trade_hash> --type gasless --chain base --poll -o json-envelope
```

- Exits 0 when confirmed (`data.successful: true`, `transactions[]` carries the final tx hash + explorer URL).
- Exits 11 if the trade reverted.
- `--poll-interval <seconds>` (default 5) tunes the cadence.

Always pass `--type gasless`: gasless trade hashes and cross-chain origin hashes look identical (0x + 64 hex), and auto-detection prefers cross-chain.

## Notes

- `--gasless` on Solana is rejected with exit 2 (`INPUT_INVALID`) — gasless is meta-transaction based and EVM-only.
- Gasless fees come out of the swap itself; there is no separate gas line in the result.
- `0x price --gasless` is still read-only and needs no wallet.
