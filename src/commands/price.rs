use crate::api::evm_swap::PriceResponse;
use crate::api::types::{compute_rate, format_amount, TokenAmount, TokenInfo};
use crate::chain;
use crate::cli::PriceArgs;
use crate::config;
use crate::error::{CliError, ErrorCode};
use crate::output::envelope::{Metadata, Warning};
use crate::output::human::KeyValueTable;
use crate::output::trade::SideMeta;
use crate::output::{HumanDisplay, OutputHandler};
use crate::token_cache::{resolve_pair_evm, TokenCache};
use serde::Serialize;
use std::io::{self, Write};

/// Formatted price result for output.
#[derive(Debug, Serialize)]
pub struct PriceResult {
    pub chain: String,
    pub sell_token: TokenInfo,
    pub buy_token: TokenInfo,
    /// Exact-in: the amount sold. Exact-out: the *estimated* sell amount.
    pub sell_amount: TokenAmount,
    pub buy_amount: TokenAmount,
    /// Exact-in: minimum buy after slippage. Exact-out: equals `buy_amount`
    /// (the buy side is fixed), so prefer `max_sell_amount` to reason about
    /// worst-case cost.
    pub min_buy_amount: TokenAmount,
    /// Exact-out only: worst-case sell amount after slippage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_sell_amount: Option<TokenAmount>,
    /// True when this priced an exact-out (buy-amount) request.
    pub exact_out: bool,
    pub rate: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_estimate: Option<String>,
    pub route: Vec<crate::api::types::RouteSource>,
    pub liquidity_available: bool,
}

impl HumanDisplay for PriceResult {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        // Exact-out pins the buy side: the sell figure is an estimate and the
        // meaningful guarantee is the worst-case sell (Max Sell), not Min Buy.
        let sell_label = if self.exact_out { "Sell (est)" } else { "Sell" };
        let mut rows = vec![
            (sell_label.to_string(), self.sell_amount.display()),
            (
                "Buy".to_string(),
                format!(
                    "{}{}",
                    self.buy_amount.display(),
                    self.buy_amount
                        .usd_value
                        .as_ref()
                        .map(|v| format!(" (~${v})"))
                        .unwrap_or_default()
                ),
            ),
            ("Rate".to_string(), self.rate.clone()),
        ];
        match (self.exact_out, &self.max_sell_amount) {
            (true, Some(max_sell)) => {
                rows.push(("Max Sell".to_string(), max_sell.display()));
            }
            _ => rows.push(("Min Buy".to_string(), self.min_buy_amount.display())),
        }

        if !self.route.is_empty() {
            let route_str = self
                .route
                .iter()
                .map(|s| {
                    if s.proportion.is_empty() {
                        s.name.clone()
                    } else {
                        format!("{} {}", s.proportion, s.name)
                    }
                })
                .collect::<Vec<_>>()
                .join(" → ");
            rows.push(("Route".to_string(), route_str));
        }

        if let Some(ref gas) = self.gas_estimate {
            rows.push(("Gas".to_string(), gas.clone()));
        }

        let table = KeyValueTable {
            title: format!("Price on {}", self.chain),
            rows,
            footer: if !self.liquidity_available {
                Some("⚠ Liquidity may not be available for this pair".to_string())
            } else {
                None
            },
        };

        table.display_human(writer, color)
    }
}

pub async fn run(
    args: &PriceArgs,
    output: &OutputHandler,
    global: &crate::GlobalOpts,
) -> Result<i32, CliError> {
    let config = config::load_config()?;

    // Resolve chain
    let chain_info = chain::resolve_chain(&args.chain)?;
    chain_info.reject_if_tron("price")?;

    // Validate token addresses
    chain::validate_token_address(&args.sell, chain_info)?;
    chain::validate_token_address(&args.buy, chain_info)?;

    let amount_spec = args.amount_spec();
    chain::validate_base_unit_amount(amount_spec.value())?;

    // Exact-out (--buy-amount) is only supported on the EVM Allowance Holder
    // path. Reject it before we hit Solana / gasless pricing.
    if amount_spec.is_exact_out() {
        if chain_info.is_solana() {
            return Err(super::exact_out_unsupported("Solana swaps"));
        }
        if args.gasless {
            return Err(super::exact_out_unsupported("gasless swaps"));
        }
    }

    // Agent-payment path: pay per request through the gateway instead of an
    // API key. EVM AllowanceHolder only.
    if let Some(pay) = args.pay {
        if !chain_info.is_evm() {
            return Err(super::pay_requires_evm());
        }
        if args.gasless {
            return Err(super::pay_incompatible("--gasless"));
        }
        // Reject exact-out up front (free) rather than paying for a request the
        // gateway may not support.
        if amount_spec.is_exact_out() {
            return Err(super::exact_out_unsupported("agent payments (--pay)"));
        }
        return run_paid_price(args, pay, &config, output, global, chain_info, &amount_spec).await;
    }

    let client = crate::api::client_for(global, &config, output)?;

    let spinner = output.spinner("Fetching price...");

    // Amounts are in base units (a 6-decimal token uses 1000000 = 1.0).
    // Solana / gasless only reach here for exact-in, so this is the sell side.
    let sell_amount = amount_spec.value();

    let mut metadata = Metadata::for_chain(chain_info);

    if chain_info.is_solana() {
        // Solana price: call swap-instructions and use amountOut
        let amount_in: u64 = sell_amount.parse().map_err(|_| CliError::Api {
            code: ErrorCode::InputInvalid,
            message: format!(
                "Invalid Solana amount '{}'. Must be a positive integer (lamports/base units).",
                sell_amount
            ),
            status: None,
            details: None,
            suggestion: Some(
                "Solana amounts are in lamports (9 decimals): 1000000000 = 1.0".into(),
            ),
        })?;

        // Solana price is read-only and never signs anything; use a constant
        // dummy taker so we don't prompt the OS keychain just for a price
        // check. The 0x Solana API doesn't validate taker balance on price.
        const DUMMY_SOLANA_TAKER: &str = "11111111111111111111111111111112";

        let req = crate::api::solana_swap::SolanaSwapRequest {
            token_in: args.sell.clone(),
            token_out: args.buy.clone(),
            amount_in,
            slippage_bps: 100,
            taker: DUMMY_SOLANA_TAKER.to_string(),
        };

        let resp = client.get_solana_swap(&req).await;
        if let Some(s) = spinner {
            s.finish_and_clear();
        }
        let resp = resp?;

        let sell = SideMeta::address_only(args.sell.clone());
        let buy = SideMeta::address_only(args.buy.clone());
        let buy_raw = resp.amount_out.to_string();
        let result = PriceResult {
            chain: chain_info.display_name.to_string(),
            sell_token: sell.token_info(),
            buy_token: buy.token_info(),
            sell_amount: sell.amount(sell_amount),
            buy_amount: buy.amount(&buy_raw),
            min_buy_amount: buy.amount(&buy_raw),
            max_sell_amount: None,
            exact_out: false,
            rate: compute_rate(sell_amount, &buy_raw),
            gas_estimate: None,
            route: Vec::new(),
            liquidity_available: true,
        };

        return Ok(output.emit_success("price", &result, metadata, Vec::new(), 0));
    }

    if args.gasless {
        // Use gasless pricing endpoint
        let resp = client
            .get_gasless_price(
                chain_info.evm_chain_id()?,
                &args.sell,
                &args.buy,
                sell_amount,
                None,
            )
            .await;

        if let Some(s) = spinner {
            s.finish_and_clear();
        }

        let resp = resp?;

        // Resolve token metadata for gasless price
        let rpc_url = config::try_resolve_rpc_url_with_override(
            global.rpc_url.as_deref(),
            &config,
            chain_info,
        );
        let mut cache = TokenCache::new();
        let mut warnings: Vec<Warning> = Vec::new();
        let (sell_meta, buy_meta) = resolve_pair_evm(
            &mut cache,
            rpc_url.as_deref(),
            chain_info.evm_chain_id()?,
            &resp.sell_token,
            &resp.buy_token,
            &mut warnings,
        )
        .await;
        let sell = SideMeta::from_meta(resp.sell_token.clone(), sell_meta);
        let buy = SideMeta::from_meta(resp.buy_token.clone(), buy_meta);

        let result = PriceResult {
            chain: chain_info.display_name.to_string(),
            sell_token: sell.token_info(),
            buy_token: buy.token_info(),
            sell_amount: sell.amount(&resp.sell_amount),
            buy_amount: buy.amount(&resp.buy_amount),
            min_buy_amount: buy.amount(&resp.min_buy_amount),
            max_sell_amount: None,
            exact_out: false,
            rate: compute_rate(&resp.sell_amount, &resp.buy_amount),
            gas_estimate: None, // Gasless = no gas
            route: Vec::new(),
            liquidity_available: resp.liquidity_available.unwrap_or(true),
        };

        if !result.liquidity_available {
            warnings.push(Warning {
                code: "NO_LIQUIDITY".into(),
                message: "Liquidity may not be available for this pair".into(),
            });
        }

        return Ok(output.emit_success("price", &result, metadata, warnings, 0));
    }

    // Standard EVM price
    let resp = client
        .get_evm_price(
            chain_info.evm_chain_id()?,
            &args.sell,
            &args.buy,
            &amount_spec,
            None,
        )
        .await;

    if let Some(s) = &spinner {
        s.set_message("Resolving token metadata...");
    }

    let resp = resp?;
    metadata.zid = resp.zid.clone();

    // Resolve token decimals and symbols via RPC
    let rpc_url =
        config::try_resolve_rpc_url_with_override(global.rpc_url.as_deref(), &config, chain_info);
    let mut cache = TokenCache::new();
    let mut warnings: Vec<Warning> = Vec::new();
    let (sell_meta, buy_meta) = resolve_pair_evm(
        &mut cache,
        rpc_url.as_deref(),
        chain_info.numeric_id().unwrap_or(0),
        &resp.sell_token,
        &resp.buy_token,
        &mut warnings,
    )
    .await;
    let sell = SideMeta::from_meta(resp.sell_token.clone(), sell_meta);
    let buy = SideMeta::from_meta(resp.buy_token.clone(), buy_meta);

    if let Some(s) = spinner {
        s.finish_and_clear();
    }

    let result = build_price_result(chain_info, &resp, &sell, &buy);

    if !result.liquidity_available {
        warnings.push(Warning {
            code: "NO_LIQUIDITY".into(),
            message: "Liquidity may not be available for this pair".into(),
        });
    }
    if resp
        .issues
        .as_ref()
        .map(|i| i.simulation_incomplete)
        .unwrap_or(false)
    {
        warnings.push(Warning {
            code: "SIMULATION_INCOMPLETE".into(),
            message: "Price simulation was incomplete — actual amounts may differ".into(),
        });
    }

    Ok(output.emit_success("price", &result, metadata, warnings, 0))
}

/// Fetch an indicative price through the agent gateway, paying per request via
/// x402 / MPP. Mirrors the standard EVM price flow (token metadata + render)
/// but the transport is a paid handshake, and the settlement is surfaced in
/// `metadata.payment` plus a human info line.
#[allow(clippy::too_many_arguments)]
async fn run_paid_price(
    args: &PriceArgs,
    pay: crate::cli::PaymentArg,
    config: &crate::config::types::AppConfig,
    output: &OutputHandler,
    global: &crate::GlobalOpts,
    chain_info: &chain::ChainInfo,
    amount_spec: &crate::api::types::AmountSpec,
) -> Result<i32, CliError> {
    let cap = crate::payment::max_payment_to_base_units(&args.max_payment)?;
    let signer = crate::wallet::evm::load_evm_signer(config, global.wallet.as_deref())?;

    let chain_id = chain_info.evm_chain_id()?;
    let chain_id_str = chain_id.to_string();
    let (amount_key, amount_val) = amount_spec.query_param();
    let query: Vec<(&str, &str)> = vec![
        ("chainId", chain_id_str.as_str()),
        ("sellToken", args.sell.as_str()),
        ("buyToken", args.buy.as_str()),
        (amount_key, amount_val),
    ];

    // Tempo RPC (mpp only): --tempo-rpc flag → `[rpc].tempo` config → default.
    let tempo_rpc = args
        .tempo_rpc
        .as_deref()
        .or_else(|| config.rpc.get("tempo").map(String::as_str));

    let spinner = output.spinner(&format!("Paying for price via {}...", pay.method().as_str()));
    let result = crate::payment::fetch::<PriceResponse>(
        pay.method(),
        &signer,
        false,
        &query,
        cap,
        global.timeout,
        tempo_rpc,
    )
    .await;
    if let Some(s) = &spinner {
        s.set_message("Resolving token metadata...");
    }
    let (resp, receipt) = result?;

    let rpc_url =
        config::try_resolve_rpc_url_with_override(global.rpc_url.as_deref(), config, chain_info);
    let mut cache = TokenCache::new();
    let mut warnings: Vec<Warning> = Vec::new();
    let (sell_meta, buy_meta) = resolve_pair_evm(
        &mut cache,
        rpc_url.as_deref(),
        chain_info.numeric_id().unwrap_or(0),
        &resp.sell_token,
        &resp.buy_token,
        &mut warnings,
    )
    .await;
    let sell = SideMeta::from_meta(resp.sell_token.clone(), sell_meta);
    let buy = SideMeta::from_meta(resp.buy_token.clone(), buy_meta);

    if let Some(s) = spinner {
        s.finish_and_clear();
    }

    output.info(&format!(
        "Paid via {} ({} base units USDC){}",
        receipt.method,
        receipt.amount_base_units.as_deref().unwrap_or("?"),
        receipt
            .tx_hash
            .as_deref()
            .map(|t| format!(" — tx {t}"))
            .unwrap_or_default(),
    ));

    let mut metadata = Metadata::for_chain(chain_info);
    metadata.zid = resp.zid.clone();
    metadata.payment = Some(receipt);

    let result = build_price_result(chain_info, &resp, &sell, &buy);
    if !result.liquidity_available {
        warnings.push(Warning {
            code: "NO_LIQUIDITY".into(),
            message: "Liquidity may not be available for this pair".into(),
        });
    }
    Ok(output.emit_success("price", &result, metadata, warnings, 0))
}

fn build_price_result(
    chain_info: &chain::ChainInfo,
    resp: &PriceResponse,
    sell: &SideMeta,
    buy: &SideMeta,
) -> PriceResult {
    let route = resp.route_sources();

    let gas_estimate = match (&resp.gas, &resp.gas_price) {
        (Some(gas), Some(gas_price)) => {
            // If either side fails to parse, skip the estimate entirely
            // rather than emit a misleading "0 ETH" line. The raw values are
            // still available on the response payload for callers that want
            // them.
            match (gas.parse::<u128>(), gas_price.parse::<u128>()) {
                (Ok(gas_num), Ok(price_num)) => {
                    let total_wei = gas_num.saturating_mul(price_num);
                    Some(format!(
                        "{} {} (gas: {}, price: {})",
                        format_amount(&total_wei.to_string(), 18),
                        chain_info.native_token,
                        gas,
                        gas_price
                    ))
                }
                _ => {
                    tracing::warn!(
                        gas = %gas,
                        gas_price = %gas_price,
                        "0x API returned unparseable gas / gas_price; omitting estimate"
                    );
                    None
                }
            }
        }
        _ => None,
    };

    PriceResult {
        chain: chain_info.display_name.to_string(),
        sell_token: sell.token_info(),
        buy_token: buy.token_info(),
        sell_amount: sell.amount(resp.display_sell_amount()),
        buy_amount: buy.amount(&resp.buy_amount),
        min_buy_amount: buy.amount(resp.display_min_buy_amount()),
        max_sell_amount: resp.max_sell_amount().map(|m| sell.amount(m)),
        exact_out: resp.is_exact_out(),
        rate: compute_rate(resp.display_sell_amount(), &resp.buy_amount),
        gas_estimate,
        route,
        liquidity_available: resp.liquidity_available.unwrap_or(true),
    }
}
