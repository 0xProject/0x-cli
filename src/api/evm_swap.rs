use super::ApiClient;
use crate::api::types::{Fees, Issues, RouteSource};
use crate::error::CliError;
use serde::{Deserialize, Serialize};

/// Price response from /swap/allowance-holder/price
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceResponse {
    pub sell_token: String,
    pub buy_token: String,
    pub sell_amount: String,
    pub buy_amount: String,
    pub min_buy_amount: String,
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
    #[serde(default)]
    pub route: Option<RouteInfo>,
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

/// Quote response from /swap/allowance-holder/quote
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteResponse {
    pub sell_token: String,
    pub buy_token: String,
    pub sell_amount: String,
    pub buy_amount: String,
    pub min_buy_amount: String,
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
    #[serde(default)]
    pub route: Option<RouteInfo>,
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
                        let bps_num: f64 = bps.parse().unwrap_or(0.0);
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

impl ApiClient {
    /// Get indicative price from /swap/allowance-holder/price
    pub async fn get_evm_price(
        &self,
        chain_id: u64,
        sell_token: &str,
        buy_token: &str,
        sell_amount: &str,
        taker: Option<&str>,
    ) -> Result<PriceResponse, CliError> {
        let chain_id_str = chain_id.to_string();
        let mut params: Vec<(&str, &str)> = vec![
            ("chainId", &chain_id_str),
            ("sellToken", sell_token),
            ("buyToken", buy_token),
            ("sellAmount", sell_amount),
        ];
        if let Some(taker) = taker {
            params.push(("taker", taker));
        }

        self.get("/swap/allowance-holder/price", &params).await
    }

    /// Get firm quote from /swap/allowance-holder/quote
    #[allow(clippy::too_many_arguments)]
    pub async fn get_evm_quote(
        &self,
        chain_id: u64,
        sell_token: &str,
        buy_token: &str,
        sell_amount: &str,
        taker: &str,
        slippage_bps: Option<u32>,
        recipient: Option<&str>,
    ) -> Result<QuoteResponse, CliError> {
        let chain_id_str = chain_id.to_string();
        let slippage_str = slippage_bps.unwrap_or(100).to_string();

        let mut params: Vec<(&str, &str)> = vec![
            ("chainId", &chain_id_str),
            ("sellToken", sell_token),
            ("buyToken", buy_token),
            ("sellAmount", sell_amount),
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
        assert_eq!(price.sell_amount, "100000000");
        assert_eq!(price.buy_amount, "50000000000000000");
        assert_eq!(price.liquidity_available, Some(true));

        let sources = price.route.unwrap().sources();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].name, "Uniswap_V3");
        assert_eq!(sources[0].proportion, "80%");
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
