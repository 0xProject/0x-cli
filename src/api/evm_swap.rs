use super::ApiClient;
use crate::api::types::{AmountSpec, Fees, Issues, RouteSource};
use crate::error::CliError;
use serde::{Deserialize, Serialize};

/// Price response from /swap/allowance-holder/price.
///
/// The shape differs by amount mode. Exact-in responses carry `sellAmount`,
/// `minBuyAmount`, and a flat `route`. Exact-out responses instead carry
/// `estimatedNetSellAmount`, `maxSellAmount`, a `mode: "exact-out"` tag, and
/// nest the route under `routes.forward` — the buy amount is the fixed target.
/// Fields that only exist in one mode are `Option`; use the accessor methods
/// (`display_sell_amount`, `display_min_buy_amount`, `route_sources`) rather
/// than the raw fields so both modes are handled uniformly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceResponse {
    pub sell_token: String,
    pub buy_token: String,
    pub buy_amount: String,
    /// Present for exact-in. Absent for exact-out (see `estimated_net_sell_amount`).
    #[serde(default)]
    pub sell_amount: Option<String>,
    /// Present for exact-in only — buy after slippage.
    #[serde(default)]
    pub min_buy_amount: Option<String>,
    /// Exact-out only: estimated sell amount before slippage.
    #[serde(default)]
    pub estimated_net_sell_amount: Option<String>,
    /// Exact-out only: worst-case sell amount after slippage.
    #[serde(default)]
    pub max_sell_amount: Option<String>,
    /// `"exact-in"` or `"exact-out"`. Absent on older responses (treat as exact-in).
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub gas: Option<String>,
    #[serde(default)]
    pub gas_price: Option<String>,
    #[serde(default)]
    pub block_number: Option<String>,
    #[serde(default)]
    pub allowance_target: Option<String>,
    #[serde(default)]
    pub total_network_fee: Option<String>,
    /// Flat route (exact-in).
    #[serde(default)]
    pub route: Option<RouteInfo>,
    /// Nested routes (exact-out): `forward` is the swap path.
    #[serde(default)]
    pub routes: Option<Routes>,
    #[serde(default)]
    pub fees: Option<Fees>,
    #[serde(default)]
    pub issues: Option<Issues>,
    #[serde(default)]
    pub liquidity_available: Option<bool>,
    #[serde(default)]
    pub zid: Option<String>,
    #[serde(default)]
    pub token_metadata: Option<serde_json::Value>,
}

/// Quote response from /swap/allowance-holder/quote. Same mode-dependent shape
/// as [`PriceResponse`] (see its docs), plus the executable `transaction`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteResponse {
    pub sell_token: String,
    pub buy_token: String,
    pub buy_amount: String,
    /// Present for exact-in. Absent for exact-out (see `estimated_net_sell_amount`).
    #[serde(default)]
    pub sell_amount: Option<String>,
    /// Present for exact-in only — buy after slippage.
    #[serde(default)]
    pub min_buy_amount: Option<String>,
    /// Exact-out only: estimated sell amount before slippage.
    #[serde(default)]
    pub estimated_net_sell_amount: Option<String>,
    /// Exact-out only: worst-case sell amount after slippage. This is the
    /// amount approvals must cover, since the swap can spend up to it.
    #[serde(default)]
    pub max_sell_amount: Option<String>,
    /// `"exact-in"` or `"exact-out"`. Absent on older responses (treat as exact-in).
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub gas: Option<String>,
    #[serde(default)]
    pub gas_price: Option<String>,
    #[serde(default)]
    pub block_number: Option<String>,
    #[serde(default)]
    pub allowance_target: Option<String>,
    #[serde(default)]
    pub total_network_fee: Option<String>,
    /// Flat route (exact-in).
    #[serde(default)]
    pub route: Option<RouteInfo>,
    /// Nested routes (exact-out): `forward` is the swap path.
    #[serde(default)]
    pub routes: Option<Routes>,
    #[serde(default)]
    pub fees: Option<Fees>,
    #[serde(default)]
    pub issues: Option<Issues>,
    #[serde(default)]
    pub liquidity_available: Option<bool>,
    #[serde(default)]
    pub transaction: Option<TransactionData>,
    #[serde(default)]
    pub zid: Option<String>,
    #[serde(default)]
    pub token_metadata: Option<serde_json::Value>,
}

/// Transaction data returned by the quote endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionData {
    pub to: String,
    pub data: String,
    pub value: String,
    pub gas: Option<String>,
    pub gas_price: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteInfo {
    #[serde(default)]
    pub fills: Vec<RouteFill>,
}

/// Exact-out responses nest the route as `{ forward, refund }`. `forward` is
/// the swap path we display; `refund` covers the leftover-sell return leg.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Routes {
    #[serde(default)]
    pub forward: Option<RouteInfo>,
    #[serde(default)]
    pub refund: Option<RouteInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteFill {
    pub source: String,
    #[serde(rename = "proportionBps")]
    pub proportion_bps: Option<String>,
}

impl RouteInfo {
    /// Extract route sources with proportions as display strings.
    pub fn sources(&self) -> Vec<RouteSource> {
        self.fills
            .iter()
            .map(|f| {
                let proportion = f
                    .proportion_bps
                    .as_ref()
                    .map(|bps| {
                        let bps_num: f64 = match bps.parse() {
                            Ok(n) => n,
                            Err(_) => {
                                tracing::warn!(
                                    source = %f.source,
                                    proportion_bps = %bps,
                                    "0x API returned unparseable proportionBps; defaulting to 0%"
                                );
                                0.0
                            }
                        };
                        format!("{:.0}%", bps_num / 100.0)
                    })
                    .unwrap_or_default();
                RouteSource {
                    name: f.source.clone(),
                    proportion,
                }
            })
            .collect()
    }
}

/// Mode-aware accessors shared by [`PriceResponse`] and [`QuoteResponse`].
/// Exact-in and exact-out responses store the sell amount, min-buy, and route
/// in different fields; these methods paper over that so callers never branch
/// on `mode`. Generated for both structs since they carry the same fields.
macro_rules! impl_evm_swap_view {
    ($t:ty) => {
        impl $t {
            /// True when this is an exact-out (buy-amount) response.
            pub fn is_exact_out(&self) -> bool {
                self.mode.as_deref() == Some("exact-out")
            }

            /// Sell amount to display: the exact sell (exact-in) or the
            /// estimated net sell (exact-out). `"0"` if the API omits both.
            pub fn display_sell_amount(&self) -> &str {
                self.sell_amount
                    .as_deref()
                    .or(self.estimated_net_sell_amount.as_deref())
                    .unwrap_or("0")
            }

            /// Minimum buy after slippage (exact-in), or the fixed buy amount
            /// (exact-out, where the buy side is pinned and can't slip down).
            pub fn display_min_buy_amount(&self) -> &str {
                self.min_buy_amount.as_deref().unwrap_or(&self.buy_amount)
            }

            /// Worst-case sell amount, present only for exact-out.
            pub fn max_sell_amount(&self) -> Option<&str> {
                self.max_sell_amount.as_deref()
            }

            /// Route fills regardless of mode — flat `route` (exact-in) or
            /// `routes.forward` (exact-out).
            pub fn route_sources(&self) -> Vec<RouteSource> {
                if let Some(r) = &self.route {
                    r.sources()
                } else if let Some(routes) = &self.routes {
                    routes
                        .forward
                        .as_ref()
                        .map(|f| f.sources())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                }
            }
        }
    };
}

impl_evm_swap_view!(PriceResponse);
impl_evm_swap_view!(QuoteResponse);

impl QuoteResponse {
    /// The amount an approval must cover so the swap can't fail on allowance.
    /// Exact-out can spend up to `maxSellAmount`; exact-in spends exactly
    /// `sellAmount`. Falls back to the estimated sell, then `"0"`.
    pub fn approval_sell_amount(&self) -> &str {
        self.max_sell_amount
            .as_deref()
            .or(self.sell_amount.as_deref())
            .or(self.estimated_net_sell_amount.as_deref())
            .unwrap_or("0")
    }
}

impl ApiClient {
    /// Get indicative price from /swap/allowance-holder/price.
    ///
    /// `amount` selects exact-in (`sellAmount`) or exact-out (`buyAmount`).
    pub async fn get_evm_price(
        &self,
        chain_id: u64,
        sell_token: &str,
        buy_token: &str,
        amount: &AmountSpec,
        taker: Option<&str>,
    ) -> Result<PriceResponse, CliError> {
        let chain_id_str = chain_id.to_string();
        let (amount_key, amount_val) = amount.query_param();
        let mut params: Vec<(&str, &str)> = vec![
            ("chainId", &chain_id_str),
            ("sellToken", sell_token),
            ("buyToken", buy_token),
            (amount_key, amount_val),
        ];
        if let Some(taker) = taker {
            params.push(("taker", taker));
        }

        self.get("/swap/allowance-holder/price", &params).await
    }

    /// Get firm quote from /swap/allowance-holder/quote.
    ///
    /// `amount` selects exact-in (`sellAmount`) or exact-out (`buyAmount`).
    #[allow(clippy::too_many_arguments)]
    pub async fn get_evm_quote(
        &self,
        chain_id: u64,
        sell_token: &str,
        buy_token: &str,
        amount: &AmountSpec,
        taker: &str,
        slippage_bps: Option<u32>,
        recipient: Option<&str>,
    ) -> Result<QuoteResponse, CliError> {
        let chain_id_str = chain_id.to_string();
        let slippage_str = slippage_bps.unwrap_or(100).to_string();
        let (amount_key, amount_val) = amount.query_param();

        let mut params: Vec<(&str, &str)> = vec![
            ("chainId", &chain_id_str),
            ("sellToken", sell_token),
            ("buyToken", buy_token),
            (amount_key, amount_val),
            ("taker", taker),
            ("slippageBps", &slippage_str),
        ];
        if let Some(r) = recipient {
            params.push(("recipient", r));
        }

        self.get("/swap/allowance-holder/quote", &params).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_price_response() {
        let json = r#"{
            "sellToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            "buyToken": "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2",
            "sellAmount": "100000000",
            "buyAmount": "50000000000000000",
            "minBuyAmount": "49500000000000000",
            "gas": "200000",
            "gasPrice": "30000000000",
            "liquidityAvailable": true,
            "route": {
                "fills": [
                    { "source": "Uniswap_V3", "proportionBps": "8000" },
                    { "source": "Curve", "proportionBps": "2000" }
                ]
            }
        }"#;

        let price: PriceResponse = serde_json::from_str(json).unwrap();
        assert!(!price.is_exact_out());
        assert_eq!(price.display_sell_amount(), "100000000");
        assert_eq!(price.buy_amount, "50000000000000000");
        assert_eq!(price.display_min_buy_amount(), "49500000000000000");
        assert_eq!(price.liquidity_available, Some(true));

        let sources = price.route_sources();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].name, "Uniswap_V3");
        assert_eq!(sources[0].proportion, "80%");
    }

    #[test]
    fn test_deserialize_exact_out_quote() {
        // Exact-out responses omit sellAmount/minBuyAmount and instead carry
        // estimatedNetSellAmount, maxSellAmount, mode, and routes.forward.
        let json = r#"{
            "mode": "exact-out",
            "sellToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            "buyToken": "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2",
            "buyAmount": "50000000000000000",
            "estimatedNetSellAmount": "100000000",
            "maxSellAmount": "101000000",
            "allowanceTarget": "0x0000000000001ff3684f28c67538d4d072c22734",
            "liquidityAvailable": true,
            "routes": {
                "forward": { "fills": [ { "source": "Uniswap_V3", "proportionBps": "10000" } ] },
                "refund": { "fills": [] }
            },
            "transaction": {
                "to": "0x0000000000001ff3684f28c67538d4d072c22734",
                "data": "0xabcdef",
                "value": "0"
            }
        }"#;

        let quote: QuoteResponse = serde_json::from_str(json).unwrap();
        assert!(quote.is_exact_out());
        // Buy is the fixed target; sell shows the estimate.
        assert_eq!(quote.buy_amount, "50000000000000000");
        assert_eq!(quote.display_sell_amount(), "100000000");
        // No minBuyAmount in exact-out → falls back to the (fixed) buy amount.
        assert_eq!(quote.display_min_buy_amount(), "50000000000000000");
        // Approvals must cover the worst-case sell, not the estimate.
        assert_eq!(quote.approval_sell_amount(), "101000000");
        assert_eq!(quote.max_sell_amount(), Some("101000000"));
        let sources = quote.route_sources();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name, "Uniswap_V3");
        assert!(quote.transaction.is_some());
    }

    #[test]
    fn test_amount_spec_query_param() {
        use crate::api::types::AmountSpec;
        assert_eq!(
            AmountSpec::ExactIn("100".into()).query_param(),
            ("sellAmount", "100")
        );
        assert_eq!(
            AmountSpec::ExactOut("50".into()).query_param(),
            ("buyAmount", "50")
        );
        assert!(AmountSpec::ExactOut("1".into()).is_exact_out());
        assert!(!AmountSpec::ExactIn("1".into()).is_exact_out());
    }

    #[test]
    fn test_deserialize_quote_with_transaction() {
        let json = r#"{
            "sellToken": "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48",
            "buyToken": "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2",
            "sellAmount": "100000000",
            "buyAmount": "50000000000000000",
            "minBuyAmount": "49500000000000000",
            "gas": "200000",
            "gasPrice": "30000000000",
            "liquidityAvailable": true,
            "transaction": {
                "to": "0x0000000000001ff3684f28c67538d4d072c22734",
                "data": "0xabcdef",
                "value": "0",
                "gas": "200000",
                "gasPrice": "30000000000"
            },
            "issues": {
                "allowance": {
                    "actual": "0",
                    "spender": "0x0000000000001ff3684f28c67538d4d072c22734"
                }
            }
        }"#;

        let quote: QuoteResponse = serde_json::from_str(json).unwrap();
        assert!(quote.transaction.is_some());
        let tx = quote.transaction.unwrap();
        assert_eq!(tx.to, "0x0000000000001ff3684f28c67538d4d072c22734");

        let issues = quote.issues.unwrap();
        assert!(issues.allowance.is_some());
        let allowance = issues.allowance.unwrap();
        assert_eq!(
            allowance.spender,
            "0x0000000000001ff3684f28c67538d4d072c22734"
        );
    }
}
