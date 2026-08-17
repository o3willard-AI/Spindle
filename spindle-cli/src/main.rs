//! spindle CLI binary entry point.

use clap::Parser;
use spindle_cli::Cli;

#[tokio::main]
async fn main() {
    // Initialize observability — CLI stdout may be command output, so logs go to stderr.
    let obs_config = spindle_obs::Config::from_env_stderr("operational");
    spindle_obs::init(&obs_config);

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
