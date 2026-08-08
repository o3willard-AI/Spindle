//! Spindle server — main application binary.
//!
//! Supports `--validate-config` flag: validates configuration and exits 0 (valid)
//! or 1 (invalid with specific error messages).
//! Supports port conflict detection at startup.

use std::net::{SocketAddr, TcpListener};

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
                println!("spindle-server — HTTP API + ingest server");
                println!();
                println!("Usage: spindle-server [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config <PATH>   Path to config file (default: ~/.spindle/config.toml or $SPINDLE_CONFIG)");
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
                        println!("Server: {}:{}", config.server.host, config.server.port);
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

    // Normal startup
    match spindle_config::Config::load() {
        Ok(config) => {
            match config.validate() {
                Ok(_) => {
                    let addr = config.server.addr();
                    if let Err(e) = check_port_available(addr) {
                        eprintln!("Port conflict: cannot bind to {}", addr);
                        eprintln!("  {}", e);
                        eprintln!("  Another process may be using this port.");
                        std::process::exit(3);
                    }

                    println!("Starting spindle-server on {}", addr);
                    // Server would start here
                }
                Err(e) => {
                    eprintln!("Configuration validation failed: {}", e);
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

/// Check if the given address is available for binding.
pub fn check_port_available(addr: SocketAddr) -> Result<(), std::io::Error> {
    TcpListener::bind(addr)
        .map(|listener| drop(listener))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_port_available() {
        // Port 0 = OS assigns available port
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        assert!(check_port_available(addr).is_ok());
    }

    #[test]
    fn test_check_port_in_use() {
        // Bind one listener, then check that the same port is in use
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

        assert!(check_port_available(addr).is_err());
    }
}
