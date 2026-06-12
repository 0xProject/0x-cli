use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

#[derive(Parser)]
#[command(
    name = "0x",
    version,
    about = "Trade tokens across chains using 0x APIs",
    long_about = "A CLI for trading tokens across Solana and EVM chains using 0x Protocol.\n\n\
        Supports EVM swaps, gasless swaps, Solana swaps, and cross-chain\n\
        swaps. Designed for both human traders and AI agents.",
    after_help = "ENVIRONMENT VARIABLES:\n\
        \x20   ZEROX_API_KEY            0x API key (overrides config)\n\
        \x20   ZEROX_EVM_PRIVATE_KEY    EVM private key (overrides wallet config)\n\
        \x20   ZEROX_SOLANA_KEYPAIR     Solana keypair path or base58 (overrides wallet config)\n\
        \x20   ZEROX_DEFAULT_CHAIN      Default chain (overrides config)\n\
        \x20   ZEROX_RPC_URL            RPC URL (overrides config)\n\
        \x20   NO_COLOR                 Disable colored output\n\n\
        CONFIG:\n\
        \x20   Non-secret values live in ~/.0x-config/config.toml\n\
        \x20   Wallet secrets go to the OS keyring by default (--plaintext to opt out)\n\
        \x20   Run '0x config init' for interactive setup\n\n\
        EXIT CODES:\n\
        \x20   0   Success\n\
        \x20   1   General error\n\
        \x20   2   Input error (malformed CLI args, unsupported chain)\n\
        \x20   3   Config error (missing API key, wallet)\n\
        \x20   4   Network error (retry with backoff)\n\
        \x20   5   Auth error (invalid API key, plan does not include endpoint)\n\
        \x20   6   Validation failed (no liquidity, insufficient sell-token balance, token not supported)\n\
        \x20   10  Simulation failed (transient RPC issue or real revert — inspect the error; one retry is ok)\n\
        \x20   11  Transaction reverted on-chain\n\
        \x20   12  Transaction pending (poll with '0x status')\n\
        \x20   20  User cancelled\n\
        \x20   25  Needs confirmation (run again with --yes to execute)\n\
        \x20   30  Dry-run completed successfully"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Output format (auto-detects: human for TTY, json-envelope otherwise)
    #[arg(short = 'o', long, global = true, value_enum, env = "ZEROX_OUTPUT")]
    pub output: Option<OutputFormat>,

    /// Skip all confirmation prompts
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    /// Suppress non-essential output (progress, status messages)
    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,

    /// Enable debug output on stderr
    #[arg(short = 'v', long, global = true)]
    pub verbose: bool,

    /// Simulate everything but don't sign or submit transactions
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Override the configured API key
    #[arg(long, global = true, env = "ZEROX_API_KEY", hide_env = true)]
    pub api_key: Option<String>,

    /// Override the RPC URL for this command
    #[arg(long, global = true, env = "ZEROX_RPC_URL", hide_env = true)]
    pub rpc_url: Option<String>,

    /// Use a named config profile for this command (see '0x config set')
    #[arg(long, global = true, env = "ZEROX_PROFILE", hide_env = true)]
    pub profile: Option<String>,

    /// Wallet path or name to use
    #[arg(short = 'w', long, global = true)]
    pub wallet: Option<String>,

    /// HTTP/RPC timeout in seconds
    #[arg(long, global = true, default_value = "30")]
    pub timeout: u64,

    /// Disable colored output
    #[arg(long, global = true)]
    pub no_color: bool,
}

impl Commands {
    /// Stable command name used in the JSON envelope `command` field and the
    /// error reporter. Keep in sync with the variants below.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Config { .. } => "config",
            Self::Price(_) => "price",
            Self::Swap(_) => "swap",
            Self::CrossChain(_) => "cross-chain",
            Self::Status(_) => "status",
            Self::Chains => "chains",
            Self::Completions { .. } => "completions",
            Self::Skill { .. } => "skill",
        }
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage CLI configuration
    #[command(
        long_about = "Manage CLI configuration stored in ~/.0x-config/config.toml.\n\n\
            The config file stores your API key, default chain, wallet settings,\n\
            and custom RPC endpoints. Environment variables always take precedence\n\
            over config file values."
    )]
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Get indicative token price (read-only, no execution)
    #[command(
        long_about = "Fetch an indicative price for a token swap without committing liquidity.\n\n\
            This is a read-only operation — no wallet or gas is needed. Use it to\n\
            check rates before executing a swap.",
        after_help = "EXAMPLES:\n\
            \x20   # Price check: 1 USDC → WETH on Base (USDC has 6 decimals: 1000000 = 1 USDC)\n\
            \x20   0x price --chain base \\\n\
            \x20     --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \\\n\
            \x20     --buy 0x4200000000000000000000000000000000000006 --amount 1000000\n\n\
            \x20   # JSON output for agents\n\
            \x20   0x price --chain base \\\n\
            \x20     --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \\\n\
            \x20     --buy 0x4200000000000000000000000000000000000006 --amount 1000000 -o json\n\n\
            \x20   BASE UNIT REFERENCE:\n\
            \x20     USDC (6 decimals):  1 USDC  = 1000000\n\
            \x20     WETH (18 decimals): 1 WETH  = 1000000000000000000\n\
            \x20     USDT (6 decimals):  1 USDT  = 1000000\n\n\
            RESPONSE (data field):\n\
            \x20   chain                 string  Display name of the chain\n\
            \x20   sell_token            object  {address, symbol?, decimals?}\n\
            \x20   buy_token             object  {address, symbol?, decimals?}\n\
            \x20   sell_amount           object  {raw, formatted, usd_value?}\n\
            \x20   buy_amount            object  {raw, formatted, usd_value?}\n\
            \x20   min_buy_amount        object  Minimum after slippage\n\
            \x20   rate                  string  buy/sell ratio\n\
            \x20   gas_estimate          string? Estimated gas cost (EVM only)\n\
            \x20   route                 array   [{name, proportion}]\n\
            \x20   liquidity_available   bool"
    )]
    Price(PriceArgs),

    /// Execute a token swap
    #[command(
        long_about = "Execute a token swap on a single chain.\n\n\
            Supports EVM chains (via Allowance Holder), Solana, and gasless\n\
            swaps. The CLI will fetch a quote, show a confirmation prompt,\n\
            handle token approvals, sign and submit the transaction, then\n\
            wait for confirmation.",
        after_help = "EXAMPLES:\n\
            \x20   # Swap 1 USDC for WETH on Base (1 USDC = 1000000 base units)\n\
            \x20   0x swap --chain base \\\n\
            \x20     --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \\\n\
            \x20     --buy 0x4200000000000000000000000000000000000006 --amount 1000000\n\n\
            \x20   # Solana swap: 1 SOL for USDC (1 SOL = 1000000000 lamports)\n\
            \x20   0x swap --chain solana \\\n\
            \x20     --sell So11111111111111111111111111111111111111112 \\\n\
            \x20     --buy EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v --amount 1000000000\n\n\
            \x20   # Agent-friendly: skip confirmation, JSON output\n\
            \x20   0x swap --chain base \\\n\
            \x20     --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \\\n\
            \x20     --buy 0x4200000000000000000000000000000000000006 --amount 1000000 --yes -o json\n\n\
            \x20   # Dry-run: simulate without executing\n\
            \x20   0x swap --chain base \\\n\
            \x20     --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \\\n\
            \x20     --buy 0x4200000000000000000000000000000000000006 --amount 1000000 --dry-run\n\n\
            RESPONSE (data field):\n\
            \x20   chain                 string  Display name of the chain\n\
            \x20   sell_token            object  {address, symbol?, decimals?}\n\
            \x20   buy_token             object  {address, symbol?, decimals?}\n\
            \x20   sell_amount           object  {raw, formatted, usd_value?}\n\
            \x20   buy_amount            object  {raw, formatted, usd_value?}\n\
            \x20   min_buy_amount        object  Minimum after slippage\n\
            \x20   rate                  string  buy/sell ratio\n\
            \x20   route                 array   [{name, proportion}]\n\
            \x20   tx_hash               string? On-chain transaction hash\n\
            \x20   explorer_url          string? Block-explorer link\n\
            \x20   block_number          number? Confirmation block\n\
            \x20   gas_used              string?\n\
            \x20   effective_gas_price   string?\n\
            \x20   dry_run               bool"
    )]
    Swap(SwapArgs),

    /// Execute a cross-chain swap
    #[command(
        long_about = "Execute a cross-chain token swap between different blockchains.\n\n\
            Supports EVM-to-EVM, EVM-to-Solana, and Solana-to-EVM swaps.\n\
            The CLI fetches multiple bridge quotes, lets you select one,\n\
            handles approvals, executes the origin transaction, and tracks\n\
            the bridge status until completion.",
        after_help = "EXAMPLES:\n\
            \x20   # Bridge 1 USDC from Base to Arbitrum\n\
            \x20   0x cross-chain --from base --to arbitrum \\\n\
            \x20     --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \\\n\
            \x20     --buy 0xaf88d065e77c8cC2239327C5EDb3A432268e5831 --amount 1000000\n\n\
            \x20   # Agent-friendly: auto-select best price quote\n\
            \x20   0x cross-chain --from base --to arbitrum \\\n\
            \x20     --sell 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 \\\n\
            \x20     --buy 0xaf88d065e77c8cC2239327C5EDb3A432268e5831 \\\n\
            \x20     --amount 1000000 --select-quote best-price --yes -o json\n\n\
            RESPONSE (data field):\n\
            \x20   origin_chain            string  Display name of origin\n\
            \x20   destination_chain       string  Display name of destination\n\
            \x20   sell_token              object  {address, symbol?, decimals?}\n\
            \x20   buy_token               object  {address, symbol?, decimals?}\n\
            \x20   sell_amount             object  {raw, formatted, usd_value?}\n\
            \x20   buy_amount              object  {raw, formatted, usd_value?}\n\
            \x20   bridge                  string  Bridge provider name\n\
            \x20   estimated_time_seconds  number?\n\
            \x20   status                  string  Bridge status string\n\
            \x20   terminal                bool\n\
            \x20   successful              bool\n\
            \x20   origin_tx_hash          string? Origin-chain tx hash\n\
            \x20   origin_explorer_url     string? Origin block-explorer link\n\
            \x20   dry_run                 bool"
    )]
    CrossChain(CrossChainArgs),

    /// Check transaction or trade status
    #[command(
        long_about = "Check the status of a gasless trade or cross-chain swap.\n\n\
            For gasless trades, provide the trade hash returned by the submit endpoint.\n\
            For cross-chain swaps, provide the origin transaction hash.\n\
            Use --poll to continuously monitor until completion.\n\n\
            NOTE: gasless trade hashes and cross-chain origin tx hashes both\n\
            look like 0x-prefixed 66-char hex strings. Auto-detection defaults\n\
            to cross-chain when the hash matches that shape; pass --type\n\
            explicitly when you know what you're polling.",
        after_help = "EXAMPLES:\n\
            \x20   # Check gasless trade status\n\
            \x20   0x status 0xabc123... --type gasless --chain base\n\n\
            \x20   # Poll cross-chain bridge status until complete\n\
            \x20   0x status 0xdef456... --type cross-chain --chain base --poll\n\n\
            \x20   # Check with custom poll interval\n\
            \x20   0x status 0xdef456... --type cross-chain --chain base --poll --poll-interval 10\n\n\
            RESPONSE (data field):\n\
            \x20   status           string  Raw status from the API\n\
            \x20   status_detail    string  Human-readable explanation\n\
            \x20   terminal         bool    Whether status is a final state\n\
            \x20   successful       bool    Whether the final state is success\n\
            \x20   transactions     array   [{chain_id?, chain_name?, tx_hash?, explorer_url?, timestamp?}]\n\
            \x20   failure_reason   string? Present only on failed cross-chain bridges"
    )]
    Status(StatusArgs),

    /// List supported chains
    #[command(
        long_about = "List all blockchain networks supported by the 0x CLI.\n\n\
            Shows chain ID, name, native token, and block explorer URL for each chain.",
        after_help = "RESPONSE (data field):\n\
            \x20   Array of:\n\
            \x20     id              number|string  Numeric chain id, or 'solana'\n\
            \x20     name            string         Lowercase short name\n\
            \x20     display_name    string\n\
            \x20     native_token    string\n\
            \x20     explorer_url    string\n\
            \x20     chain_type      string         'evm' | 'svm'"
    )]
    Chains,

    /// Generate shell completions
    #[command(
        long_about = "Generate shell completion scripts.\n\n\
            Output the completion script to stdout for the specified shell.",
        after_help = "EXAMPLES:\n\
            \x20   # Bash\n\
            \x20   0x completions bash > ~/.bash_completion.d/0x\n\n\
            \x20   # Zsh\n\
            \x20   0x completions zsh > ~/.zfunc/_0x\n\n\
            \x20   # Fish\n\
            \x20   0x completions fish > ~/.config/fish/completions/0x.fish"
    )]
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },

    /// Print or install the bundled agent skill
    #[command(
        long_about = "Print or install the bundled agent skill — markdown\n\
            documents describing how an AI agent should use this CLI\n\
            (commands, output contract, exit codes, gotchas). The skill is\n\
            compiled into the binary so it always matches this version.\n\n\
            The skill is one SKILL.md entry point plus deep-dive reference\n\
            topics (gasless, cross-chain, solana, config, tokens, errors)\n\
            that agents read on demand.",
        after_help = "EXAMPLES:\n\
            \x20   # Print the main skill to stdout\n\
            \x20   0x skill print\n\n\
            \x20   # Print one reference topic\n\
            \x20   0x skill print --topic errors\n\n\
            \x20   # Install SKILL.md + references/ into ./.claude/skills/0x-trade/\n\
            \x20   0x skill install\n\n\
            \x20   # Install into a custom skills directory\n\
            \x20   0x skill install --dir ~/.claude/skills\n\n\
            NOTE: `skill print` writes raw markdown to stdout — the global\n\
            -o/--output flag is ignored for this command."
    )]
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
}

#[derive(Subcommand)]
pub enum SkillAction {
    /// Write the embedded skill markdown to stdout
    Print {
        /// Print a reference topic instead of the main SKILL.md
        #[arg(long, value_enum)]
        topic: Option<SkillTopic>,
    },
    /// Write the full skill directory (SKILL.md + references/) to disk
    Install {
        /// Skills directory to install into (default: ./.claude/skills)
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SkillTopic {
    /// Gasless swaps and trade-hash polling
    Gasless,
    /// Cross-chain swaps, quote selection, bridge status
    CrossChain,
    /// Solana swaps and flag differences
    Solana,
    /// Config, wallets, keyring, env vars
    Config,
    /// Supported chains and token addresses
    Tokens,
    /// Error catalog and recovery playbook
    Errors,
}

impl SkillTopic {
    /// File stem of the bundled reference (`references/<stem>.md`).
    pub fn file_stem(&self) -> &'static str {
        match self {
            Self::Gasless => "gasless",
            Self::CrossChain => "cross-chain",
            Self::Solana => "solana",
            Self::Config => "config",
            Self::Tokens => "tokens",
            Self::Errors => "errors",
        }
    }
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Interactive configuration wizard
    #[command(long_about = "Interactively set up your 0x CLI configuration.\n\n\
        Guides you through setting your API key, default chain, wallet,\n\
        and RPC endpoints. Pass --browser to open dashboard.0x.org in your\n\
        default browser so you can grab an API key without leaving the\n\
        terminal.")]
    Init {
        /// Open https://dashboard.0x.org in the default browser to grab an API key
        #[arg(long)]
        browser: bool,
    },

    /// Set a configuration value
    #[command(
        long_about = "Set a configuration value.\n\n\
            Wallet secrets (wallet.evm, wallet.solana with key material) are\n\
            stored in the OS keyring by default — pass --plaintext to keep\n\
            them in the config file instead. Solana wallet values that look\n\
            like a path (contain / or end in .json) always stay in the config\n\
            file because the path itself isn't sensitive.",
        after_help = "KEYS:\n\
            \x20   api_key                  Your 0x API key (config file)\n\
            \x20   defaults.chain           Default chain (e.g. 'base', '8453')\n\
            \x20   defaults.slippage_bps    Default slippage in basis points\n\
            \x20   defaults.approval_type   Token approval type: 'exact' or 'unlimited'\n\
            \x20   rpc.<chain>              RPC URL for a chain (e.g. rpc.base)\n\
            \x20   wallet.evm               EVM private key (hex) — secret, → keyring\n\
            \x20   wallet.solana            Solana keypair: file path (config file)\n\
            \x20                            OR base58/JSON-array secret (→ keyring)\n\n\
            EXAMPLES:\n\
            \x20   0x config set api_key abc123def456\n\
            \x20   0x config set defaults.chain base\n\
            \x20   0x config set rpc.base https://base.llamarpc.com\n\
            \x20   0x config set wallet.evm 0xac09...                    # → keyring\n\
            \x20   0x config set wallet.evm 0xac09... --plaintext        # → config file\n\
            \x20   0x config set wallet.solana /path/to/keypair.json     # → config file\n\n\
            RESPONSE (data field):\n\
            \x20   key      string  The configuration key that was set\n\
            \x20   value    string  The stored value (redacted for secrets)\n\
            \x20   storage  string  'keyring' | 'config'"
    )]
    Set {
        /// Configuration key (dot-notation supported)
        key: String,
        /// Value to set
        value: String,
        /// Store wallet secrets in the config file instead of the OS keyring.
        /// Has no effect on non-secret keys (e.g. api_key, defaults.*, rpc.*).
        #[arg(long)]
        plaintext: bool,
    },

    /// Get a configuration value
    Get {
        /// Configuration key (dot-notation supported)
        key: String,
    },

    /// Remove a configuration value (clears both config file and OS keyring entries)
    #[command(
        long_about = "Remove a configuration value.\n\n\
            For wallet keys, deletes the entry from both the config file and the\n\
            OS keyring. For other keys, just clears the config file entry.",
        after_help = "EXAMPLES:\n\
            \x20   0x config unset wallet.evm        # delete EVM wallet (keyring + config)\n\
            \x20   0x config unset wallet.solana     # delete Solana wallet\n\
            \x20   0x config unset rpc.base          # remove custom Base RPC override\n\n\
            RESPONSE (data field):\n\
            \x20   key      string  The configuration key that was targeted\n\
            \x20   changed  bool    true if something was actually removed"
    )]
    Unset {
        /// Configuration key (dot-notation supported)
        key: String,
    },

    /// Show full configuration (secrets redacted)
    #[command(
        long_about = "Print the active configuration. Wallet secrets are\n\
            redacted; entries stored in the OS keyring are reported as\n\
            '<stored in keyring>'.",
        after_help = "RESPONSE (data field):\n\
            \x20   api.api_key             string?  Redacted (e.g. 'abcd...wxyz')\n\
            \x20   defaults.chain          string?\n\
            \x20   defaults.slippage_bps   number\n\
            \x20   defaults.approval_type  string\n\
            \x20   rpc                     object   {chain_name: rpc_url}\n\
            \x20   wallet.evm              string?  '***redacted***' | '<stored in keyring>'\n\
            \x20   wallet.solana           string?  Path verbatim, or '***redacted***' / '<stored in keyring>'"
    )]
    Show,

    /// Print the configuration directory path
    Path,
}

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// Colored, table-based output for humans
    Human,
    /// Just the data field (for piping)
    Json,
    /// Full envelope with metadata, warnings, errors
    JsonEnvelope,
}

use serde::Serialize;

impl OutputFormat {
    /// Auto-detect: human if TTY, json-envelope otherwise
    pub fn detect(explicit: Option<OutputFormat>) -> Self {
        use std::io::IsTerminal;
        match explicit {
            Some(f) => f,
            None => {
                if std::io::stdout().is_terminal() {
                    OutputFormat::Human
                } else {
                    OutputFormat::JsonEnvelope
                }
            }
        }
    }
}

#[derive(Parser, Debug)]
pub struct PriceArgs {
    /// Chain ID or name (e.g. base, 8453, ethereum, solana)
    #[arg(short = 'c', long, env = "ZEROX_DEFAULT_CHAIN")]
    pub chain: String,

    /// Token to sell (contract address, e.g. 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913)
    #[arg(long)]
    pub sell: String,

    /// Token to buy (contract address, e.g. 0x4200000000000000000000000000000000000006)
    #[arg(long)]
    pub buy: String,

    /// Amount to sell in base units (e.g. 1000000 = 1 USDC, 1000000000000000000 = 1 ETH)
    #[arg(long)]
    pub amount: String,

    /// Use gasless pricing
    #[arg(long)]
    pub gasless: bool,
}

#[derive(Parser, Debug)]
pub struct SwapArgs {
    /// Chain ID or name (e.g. base, 8453, ethereum, solana)
    #[arg(short = 'c', long, env = "ZEROX_DEFAULT_CHAIN")]
    pub chain: String,

    /// Token to sell (contract address)
    #[arg(long)]
    pub sell: String,

    /// Token to buy (contract address)
    #[arg(long)]
    pub buy: String,

    /// Amount to sell in base units (e.g. 1000000 = 1 USDC, 1000000000000000000 = 1 ETH)
    #[arg(long)]
    pub amount: String,

    /// Slippage tolerance in basis points (100 = 1%, max 10000 = 100%)
    #[arg(long, default_value = "100", value_parser = clap::value_parser!(u32).range(0..=10000))]
    pub slippage: u32,

    /// Use gasless swap (no gas needed; EVM only — rejected on Solana)
    #[arg(long)]
    pub gasless: bool,

    /// Send output tokens to a different address (EVM only — rejected on Solana)
    #[arg(long)]
    pub recipient: Option<String>,

    /// Token approval strategy (EVM only — warns and is ignored on Solana)
    #[arg(long, value_enum, default_value = "exact")]
    pub approval: ApprovalStrategy,
}

#[derive(Parser, Debug)]
pub struct CrossChainArgs {
    /// Origin chain ID or name
    #[arg(long)]
    pub from: String,

    /// Destination chain ID or name
    #[arg(long)]
    pub to: String,

    /// Token to sell (contract address)
    #[arg(long)]
    pub sell: String,

    /// Token to buy (contract address)
    #[arg(long)]
    pub buy: String,

    /// Amount to sell in base units (e.g. 1000000 = 1 USDC, 1000000000000000000 = 1 ETH)
    #[arg(long)]
    pub amount: String,

    /// Slippage tolerance in basis points (100 = 1%, max 10000 = 100%)
    #[arg(long, default_value = "100", value_parser = clap::value_parser!(u32).range(0..=10000))]
    pub slippage: u32,

    /// Sort quotes by price or speed
    #[arg(long, value_enum, default_value = "price")]
    pub sort: QuoteSort,

    /// Select quote (index, 'best-price', or 'fastest')
    #[arg(long)]
    pub select_quote: Option<String>,

    /// Maximum number of quotes to fetch (1-10)
    #[arg(long, default_value = "3")]
    pub max_quotes: u8,
}

#[derive(Parser, Debug)]
pub struct StatusArgs {
    /// Transaction hash or trade hash
    pub hash: String,

    /// Chain ID or name (required for some status checks)
    #[arg(short = 'c', long)]
    pub chain: Option<String>,

    /// Status type (auto-detected if omitted)
    #[arg(long, value_enum, rename_all = "kebab-case")]
    pub r#type: Option<StatusType>,

    /// Continuously poll until terminal state
    #[arg(long)]
    pub poll: bool,

    /// Poll interval in seconds (minimum 1)
    #[arg(long, default_value = "5", value_parser = clap::value_parser!(u64).range(1..))]
    pub poll_interval: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalStrategy {
    /// Approve only the exact amount needed
    Exact,
    /// Approve unlimited (max uint256)
    Unlimited,
}

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuoteSort {
    /// Sort by best price
    Price,
    /// Sort by fastest estimated time
    Speed,
}

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StatusType {
    /// Gasless trade status
    Gasless,
    /// Cross-chain bridge status
    CrossChain,
}
