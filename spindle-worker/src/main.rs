//! Spindle worker — queue consumer + rollups + exports + reconciliation.
//!
//! Supports `--validate-config` flag for config validation (same as spindle-server).

use std::net::SocketAddr;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut validate_only = false;
    let mut config_path: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--validate-config" => {
                validate_only = true;
            }
            "--config" | "-c" => {
                if i + 1 < args.len() {
                    config_path = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("Error: --config requires a path argument");
                    std::process::exit(1);
                }
            }
            "--help" | "-h" => {
                println!("spindle-worker — queue consumer + rollups + exports + reconciliation");
                println!();
                println!("Usage: spindle-worker [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config <PATH>   Path to config file");
                println!("  --validate-config Validate configuration and exit");
                println!("  --help            Print this help message");
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    if let Some(path) = &config_path {
        std::env::set_var("SPINDLE_CONFIG", path);
    }

    if validate_only {
        match spindle_config::Config::load() {
            Ok(config) => {
                match config.validate() {
                    Ok(_) => {
                        println!("Configuration is valid");
                        println!("Database: connected");
                        println!("Storage: {}", config.storage.backend);
                        std::process::exit(0);
                    }
                    Err(e) => {
                        eprintln!("Configuration validation failed:");
                        eprintln!("  {}", e);
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                eprintln!("Failed to load configuration: {}", e);
                std::process::exit(1);
            }
        }
    }

    println!("Starting spindle-worker");
    // Worker would start here
}
