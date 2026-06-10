# Chains and token reference

Run `0x chains -o json-envelope` for the live list (with explorer URLs and chain types). Snapshot:

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
| 42161 | arbitrum | Arbitrum | ETH |
| 43114 | avalanche | Avalanche | AVAX |
| 57073 | ink | Ink | ETH |
| 59144 | linea | Linea | ETH |
| 80094 | berachain | Berachain | BERA |
| 81457 | blast | Blast | ETH |
| 534352 | scroll | Scroll | ETH |
| solana | solana | Solana | SOL |

`--chain` accepts either the numeric ID or the lowercase name.

## Common tokens

Addresses are **chain-specific** — USDC on Base and USDC on Ethereum are different contracts. Verify on a block explorer before moving material value.

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

## Base-unit cheat sheet

| Token decimals | 1 token in base units |
|---:|---|
| 6 (USDC, USDT) | `1000000` |
| 8 (cbBTC, WBTC) | `100000000` |
| 9 (SOL) | `1000000000` |
| 18 (ETH, WETH, most ERC-20s) | `1000000000000000000` |

When in doubt, run a `0x price` first — the response's `formatted` fields confirm the decimals interpretation before any value moves.
