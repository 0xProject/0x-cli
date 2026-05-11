mod api;
mod chain;
mod cli;
mod commands;
mod config;
mod confirm;
mod error;
mod output;
mod token_cache;
mod wallet;

use clap::Parser;
use cli::{Cli, Commands, ConfigAction, OutputFormat};
use output::envelope::Metadata;
use output::OutputHandler;
use std::io::IsTerminal;

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

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Determine output format (auto-detect TTY)
    let format = OutputFormat::detect(cli.output);

    // Determine color (disabled if --no-color, NO_COLOR env, or non-TTY)
    let color = !cli.no_color
        && std::env::var("NO_COLOR").is_err()
        && std::io::stdout().is_terminal()
        && matches!(format, OutputFormat::Human);

    let output = OutputHandler::new(format, color, cli.quiet);
    let global = GlobalOpts::from(&cli);

    let exit_code = match run_command(&cli, &output, &global).await {
        Ok(code) => code,
        Err(err) => {
            let command_name = match &cli.command {
                Commands::Config { .. } => "config",
                Commands::Price(_) => "price",
                Commands::Swap(_) => "swap",
                Commands::CrossChain(_) => "cross-chain",
                Commands::Status(_) => "status",
                Commands::Chains => "chains",
                Commands::Completions { .. } => "completions",
            };
            output.error(command_name, &err, Metadata::default())
        }
    };

    std::process::exit(exit_code);
}

async fn run_command(
    cli: &Cli,
    output: &OutputHandler,
    global: &GlobalOpts,
) -> Result<i32, error::CliError> {
    match &cli.command {
        Commands::Config { action } => match action {
            ConfigAction::Init => commands::config_cmd::run_init(output),
            ConfigAction::Set { key, value, plaintext } => {
                commands::config_cmd::run_set(key, value, *plaintext, output)
            }
            ConfigAction::Get { key } => commands::config_cmd::run_get(key, output),
            ConfigAction::Unset { key } => commands::config_cmd::run_unset(key, output),
            ConfigAction::Show => commands::config_cmd::run_show(output),
            ConfigAction::Path => commands::config_cmd::run_path(output),
        },
        Commands::Price(args) => commands::price::run(args, output, global).await,
        Commands::Swap(args) => commands::swap::run(args, output, global).await,
        Commands::CrossChain(args) => commands::cross_chain::run(args, output, global).await,
        Commands::Status(args) => commands::status::run(args, output, global).await,
        Commands::Chains => commands::chains::run(output),
        Commands::Completions { shell } => {
            let mut cmd = <Cli as clap::CommandFactory>::command();
            clap_complete::generate(*shell, &mut cmd, "0x", &mut std::io::stdout());
            Ok(0)
        }
    }
}
