mod cli;
mod commands;
mod config;
mod environment;
mod platform;
mod store;

use clap::Parser;
use cli::Cli;

fn main() {
    if let Err(error) = commands::execute(Cli::parse()) {
        eprintln!("panman: {error:#}");
        std::process::exit(1);
    }
}
