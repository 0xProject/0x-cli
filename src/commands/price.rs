use crate::api::evm_swap::PriceResponse;
use crate::api::types::{compute_rate, format_amount, TokenAmount, TokenInfo};
use crate::api::ApiClient;
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
    pub sell_amount: TokenAmount,
    pub buy_amount: TokenAmount,
    pub min_buy_amount: TokenAmount,
    pub rate: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_estimate: Option<String>,
    pub route: Vec<crate::api::types::RouteSource>,
    pub liquidity_available: bool,
}

impl HumanDisplay for PriceResult {
    fn display_human(&self, writer: &mut dyn Write, color: bool) -> io::Result<()> {
        let mut rows = vec![
            ("Sell".to_string(), self.sell_amount.display()),
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
            ("Min Buy".to_string(), self.min_buy_amount.display()),
        ];

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

    // Resolve API key: CLI flag > config > error
    let api_key = global
        .api_key
        .as_deref()
        .or(config.api.api_key.as_deref())
        .ok_or_else(CliError::api_key_missing)?
        .to_string();

    // Resolve chain
    let chain_info = chain::resolve_chain(&args.chain)?;

    // Validate token addresses
    chain::validate_token_address(&args.sell, chain_info)?;
    chain::validate_token_address(&args.buy, chain_info)?;
    chain::validate_base_unit_amount(&args.amount)?;

    let client = ApiClient::new(api_key, global.timeout)?;

    let spinner = output.spinner("Fetching price...");

    // Amounts are in base units (e.g. 1000000 = 1 USDC with 6 decimals)
    let sell_amount = &args.amount;

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
            suggestion: Some("For SOL, 1 SOL = 1000000000 lamports".into()),
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
                chain_info.numeric_id().unwrap(),
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
            chain_info.numeric_id().expect("EVM chain has a numeric id"),
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
            chain_info.numeric_id().unwrap(),
            &args.sell,
            &args.buy,
            sell_amount,
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

fn build_price_result(
    chain_info: &chain::ChainInfo,
    resp: &PriceResponse,
    sell: &SideMeta,
    buy: &SideMeta,
) -> PriceResult {
    let route = resp.route.as_ref().map(|r| r.sources()).unwrap_or_default();

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
        sell_amount: sell.amount(&resp.sell_amount),
        buy_amount: buy.amount(&resp.buy_amount),
        min_buy_amount: buy.amount(&resp.min_buy_amount),
        rate: compute_rate(&resp.sell_amount, &resp.buy_amount),
        gas_estimate,
        route,
        liquidity_available: resp.liquidity_available.unwrap_or(true),
    }
}
