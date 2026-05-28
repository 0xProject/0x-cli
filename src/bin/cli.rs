use clap::Parser;
use zero_x_cli::cli::{Cli, Commands, ConfigAction, SkillAction};
use zero_x_cli::output::envelope::Metadata;
use zero_x_cli::output::OutputHandler;
use zero_x_cli::{cli::OutputFormat, commands, error, GlobalOpts};
use std::io::IsTerminal;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    init_tracing(cli.verbose);

    let format = OutputFormat::detect(cli.output);
    let color = !cli.no_color
        && std::env::var("NO_COLOR").is_err()
        && std::io::stdout().is_terminal()
        && matches!(format, OutputFormat::Human);

    let output = OutputHandler::new(format, color, cli.quiet);
    let global = GlobalOpts::from(&cli);

    let exit_code = match run_command(&cli, &output, &global).await {
        Ok(code) => code,
        Err(err) => output.error(cli.command.name(), &err, Metadata::default()),
    };

    std::process::exit(exit_code);
}

fn init_tracing(verbose: bool) {
    use tracing_subscriber::{fmt, EnvFilter};

    // Honor RUST_LOG when set; otherwise default to "warn" (or "debug" with --verbose).
    // CLI logs go to stderr so they don't pollute the JSON envelope on stdout.
    let default_level = if verbose { "debug" } else { "warn" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
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
        Commands::Skill { action } => match action {
            SkillAction::Print => commands::skill::run_print(),
        },
    }
}
