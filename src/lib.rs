pub mod api;
pub mod chain;
pub mod cli;
pub mod commands;
pub mod config;
pub mod confirm;
pub mod error;
pub mod output;
pub mod token_cache;
pub mod wallet;

use cli::Cli;

/// Global options extracted from CLI flags, passed to all commands.
pub struct GlobalOpts {
    pub api_key: Option<String>,
    pub wallet: Option<String>,
    pub rpc_url: Option<String>,
    pub timeout: u64,
    pub yes: bool,
    pub dry_run: bool,
    pub verbose: bool,
}

impl From<&Cli> for GlobalOpts {
    fn from(cli: &Cli) -> Self {
        Self {
            api_key: cli.api_key.clone(),
            wallet: cli.wallet.clone(),
            rpc_url: cli.rpc_url.clone(),
            timeout: cli.timeout,
            yes: cli.yes,
            dry_run: cli.dry_run,
            verbose: cli.verbose,
        }
    }
}
