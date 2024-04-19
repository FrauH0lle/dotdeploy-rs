use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Deploy system configuration or modules
    Deploy { modules: Option<Vec<String>> },
    /// Remove system configuration or modules
    Remove { modules: Option<Vec<String>> },
}

pub(crate) fn get_cli() -> Cli {
    let cli = Cli::parse();
    cli
}
