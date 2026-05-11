use crate::chain::ChainsList;
use crate::error::CliError;
use crate::output::OutputHandler;
use crate::output::envelope::Metadata;

pub fn run(output: &OutputHandler) -> Result<i32, CliError> {
    let data = ChainsList;
    output
        .success("chains", &data, Metadata::default(), Vec::new())
        .map_err(|e| CliError::config(crate::error::ErrorCode::Unknown, e.to_string()))
}
