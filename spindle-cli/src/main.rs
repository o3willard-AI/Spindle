//! spindle CLI binary entry point.

use clap::Parser;
use spindle_cli::Cli;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match spindle_cli::run_cli(cli).await {
        Ok(output) => {
            print!("{}", output);
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
