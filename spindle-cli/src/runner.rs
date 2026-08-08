//! Command execution logic.

use serde_json::Value;

use crate::client::ApiClient;
use crate::cli_def::{Cli, Commands, NodeCmd, RunCmd, ComplianceCmd, WaiverCmd, CookbookCmd};
use crate::config::CliConfig;

pub async fn run(cli: Cli) -> Result<String, Box<dyn std::error::Error>> {
    let config = CliConfig::load(cli.config.as_ref());

    let output = match &cli.command {
        Commands::Nodes { cmd } => {
            let server = cli.resolve_server(&config)?;
            let token = cli.resolve_token(&config)?;
            execute_node_cmd(cmd, &server, &token, &cli).await?
        }
        Commands::Runs { cmd } => {
            let server = cli.resolve_server(&config)?;
            let token = cli.resolve_token(&config)?;
            execute_run_cmd(cmd, &server, &token, &cli).await?
        }
        Commands::Compliance { cmd } => {
            let server = cli.resolve_server(&config)?;
            let token = cli.resolve_token(&config)?;
            execute_compliance_cmd(cmd, &server, &token, &cli).await?
        }
        Commands::Waivers { cmd } => {
            let server = cli.resolve_server(&config)?;
            let token = cli.resolve_token(&config)?;
            execute_waiver_cmd(cmd, &server, &token, &cli).await?
        }
        Commands::Cookbooks { cmd } => {
            let server = cli.resolve_server(&config)?;
            let token = cli.resolve_token(&config)?;
            execute_cookbook_cmd(cmd, &server, &token, &cli).await?
        }
        Commands::Health => {
            let server = cli.resolve_server(&config).unwrap_or_else(|_| "http://localhost:3000".to_string());
            execute_health(&server, &cli).await?
        }
        Commands::Metrics => {
            let server = cli.resolve_server(&config).unwrap_or_else(|_| "http://localhost:3000".to_string());
            let token = cli.resolve_token(&config).unwrap_or_default();
            execute_metrics(&server, &token, &cli).await?
        }
    };

    Ok(output)
}

async fn execute_node_cmd(
    cmd: &NodeCmd,
    server: &str,
    token: &str,
    cli: &Cli,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = ApiClient::new(server, token);
    let output = match cmd {
        NodeCmd::List => {
            let data = client.get_json("v1/nodes").await?;
            cli.format_output(data)
        }
        NodeCmd::Get { id } => {
            let data = client.get_json(&format!("v1/nodes/{}", id)).await?;
            cli.format_output(data)
        }
        NodeCmd::State { id } => {
            let data = client.get_json(&format!("v1/nodes/{}/state", id)).await?;
            cli.format_output(data)
        }
    };
    Ok(output)
}

async fn execute_run_cmd(
    cmd: &RunCmd,
    server: &str,
    token: &str,
    cli: &Cli,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = ApiClient::new(server, token);
    let output = match cmd {
        RunCmd::List { node } => {
            let path = if let Some(n) = node {
                format!("v1/runs?node_id={}", n)
            } else {
                "v1/runs".to_string()
            };
            let data = client.get_json(&path).await?;
            cli.format_output(data)
        }
        RunCmd::Get { id } => {
            let data = client.get_json(&format!("v1/runs/{}", id)).await?;
            cli.format_output(data)
        }
    };
    Ok(output)
}

async fn execute_compliance_cmd(
    cmd: &ComplianceCmd,
    server: &str,
    token: &str,
    cli: &Cli,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = ApiClient::new(server, token);
    let output = match cmd {
        ComplianceCmd::Reports => {
            let data = client.get_json("v1/compliance/reports").await?;
            cli.format_output(data)
        }
        ComplianceCmd::Controls { node } => {
            let path = if let Some(n) = node {
                format!("v1/compliance/controls?node_id={}", n)
            } else {
                "v1/compliance/controls".to_string()
            };
            let data = client.get_json(&path).await?;
            cli.format_output(data)
        }
        ComplianceCmd::Export { report_type, format } => {
            let path = format!(
                "v1/compliance/export/{}?format={}",
                report_type, format
            );
            let data = client.get_json(&path).await?;
            cli.format_output(data)
        }
    };
    Ok(output)
}

async fn execute_waiver_cmd(
    cmd: &WaiverCmd,
    server: &str,
    token: &str,
    cli: &Cli,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = ApiClient::new(server, token);
    let output = match cmd {
        WaiverCmd::List => {
            let data = client.get_json("v1/waivers").await?;
            cli.format_output(data)
        }
        WaiverCmd::Get { id } => {
            let data = client.get_json(&format!("v1/waivers/{}", id)).await?;
            cli.format_output(data)
        }
        WaiverCmd::Create { control_id, profile_id, justification, approver, days } => {
            let body = serde_json::json!({
                "control_id": control_id,
                "profile_id": profile_id,
                "justification": justification,
                "approver": approver,
                "expiry_days": days,
            });
            let data = client.post_json("v1/waivers", &body).await?;
            cli.format_output(data)
        }
        WaiverCmd::Update { id, justification, approver, days } => {
            let mut body = serde_json::Map::new();
            if let Some(j) = justification { body.insert("justification".to_string(), Value::String(j.clone())); }
            if let Some(a) = approver { body.insert("approver".to_string(), Value::String(a.clone())); }
            if let Some(d) = days { body.insert("expiry_days".to_string(), Value::Number((*d).into())); }
            let data = client.patch_json(&format!("v1/waivers/{}", id), &Value::Object(body)).await?;
            cli.format_output(data)
        }
        WaiverCmd::Delete { id } => {
            let status = client.delete(&format!("v1/waivers/{}", id)).await?;
            cli.format_output(serde_json::json!({
                "status": "deleted",
                "id": id,
                "http_status": status
            }))
        }
    };
    Ok(output)
}

async fn execute_cookbook_cmd(
    cmd: &CookbookCmd,
    server: &str,
    token: &str,
    cli: &Cli,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = ApiClient::new(server, token);
    let output = match cmd {
        CookbookCmd::List => {
            let data = client.get_json("v1/cookbooks").await?;
            cli.format_output(data)
        }
    };
    Ok(output)
}

async fn execute_health(
    server: &str,
    cli: &Cli,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = ApiClient::new(server, "");
    let result = client.health_check().await;

    match result {
        Ok(data) => {
            let output = cli.format_output(data);
            Ok(output)
        }
        Err(_) => {
            let err_output = cli.format_output(serde_json::json!({
                "status": "unhealthy",
                "server": server
            }));
            Ok(err_output)
        }
    }
}

async fn execute_metrics(
    server: &str,
    token: &str,
    cli: &Cli,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = ApiClient::new(server, token);
    let data = client.get_json("v1/metrics").await?;
    Ok(cli.format_output(data))
}
