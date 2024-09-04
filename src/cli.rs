use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,
    /// Verbosity level - specify up to 2 times to get more detailed output.
    #[clap(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    pub(crate) verbosity: u8,
    /// Skip package installation during deployment
    #[clap(long, short, action)]
    pub(crate) skip_pkg_install: bool,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Deploy system configuration or modules
    Deploy { modules: Option<Vec<String>> },
    /// Remove system configuration or modules
    Remove { modules: Option<Vec<String>> },
}

pub(crate) fn get_cli() -> Cli {
    let mut cli = Cli::parse();
    cli.verbosity = std::cmp::min(2, cli.verbosity);
    cli
}
