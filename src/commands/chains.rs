use crate::chain::ChainsList;
use crate::error::CliError;
use crate::output::envelope::Metadata;
use crate::output::OutputHandler;

pub fn run(output: &OutputHandler) -> Result<i32, CliError> {
    let data = ChainsList;
    Ok(output.emit_success("chains", &data, Metadata::default(), Vec::new(), 0))
}
