//! spindle CLI binary entry point.

use clap::Parser;
use spindle_cli::Cli;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match spindle_cli::run_cli(cli).await {
        Ok((output, code)) => {
            print!("{}", output);
            std::process::exit(code);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
