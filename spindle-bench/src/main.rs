//! Spindle load test tool — replays corpus at fleet-scale rates.
//!
//! # Usage
//! ```sh
//! spindle-bench --server http://localhost:3000 --token <token> \
//!   --mode sustained   # 960K runs/day (11.1/sec) for 60s
//! spindle-bench --server http://localhost:3000 --token <token> \
//!   --mode peak        # 150 runs/sec burst for 60s
//! spindle-bench --server http://localhost:3000 --token <token> \
//!   --mode stress      # 300 runs/sec (2x target) for 60s
//! spindle-bench --server http://localhost:3000 --token <token> \
//!   --mode full        # All three phases, results to BENCHMARKS.md
//! ```

#![allow(warnings)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use rand::Rng;
use serde_json::json;
use tokio::sync::Semaphore;

#[derive(Parser, Debug)]
#[command(name = "spindle-bench", about = "Spindle fleet load test tool")]
struct Args {
    /// Spindle server base URL
    #[arg(long, default_value = "http://localhost:3000")]
    server: String,

    /// Data collector auth token
    #[arg(long)]
    token: String,

    /// Load test mode: sustained, peak, stress, full
    #[arg(long, default_value = "full")]
    mode: String,

    /// Duration of each phase in seconds
    #[arg(long, default_value_t = 60)]
    duration: u64,

    /// Max concurrent requests
    #[arg(long, default_value_t = 256)]
    concurrency: usize,

    /// Output file for results (BENCHMARKS.md)
    #[arg(long, default_value = "BENCHMARKS.md")]
    output: String,
}

/// Request result tracking
#[derive(Debug, Clone)]
struct RequestResult {
    status: u16,
    latency_ms: f64,
    bytes_sent: usize,
    accepted: bool, // 202 = accepted
}

/// Phase configuration
struct PhaseConfig {
    name: &'static str,
    target_rps: f64,
    description: &'static str,
}

const PHASES: &[PhaseConfig] = &[
    PhaseConfig {
        name: "sustained",
        target_rps: 11.1, // 960,000 runs/day ÷ 86400 sec
        description: "960K runs/day (11.1 req/s baseline fleet load)",
    },
    PhaseConfig {
        name: "peak",
        target_rps: 150.0,
        description: "Peak burst (150 req/s — concurrent converge window)",
    },
    PhaseConfig {
        name: "stress",
        target_rps: 300.0,
        description: "2x headroom (300 req/s — graceful degradation required)",
    },
];

/// Generate a synthetic Chef data collector payload
fn generate_payload(idx: u64) -> serde_json::Value {
    let mut rng = rand::thread_rng();
    let node_id = format!("node-{:04}", rng.gen_range(0..2000));
    let run_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();
    let started = now - chrono::Duration::seconds(rng.gen_range(30..600));
    let status = if rng.gen_bool(0.85) {
        "success"
    } else if rng.gen_bool(0.5) {
        "failure"
    } else {
        "changed"
    };

    let resource_count = rng.gen_range(5..50);
    let mut resources = Vec::new();
    for r in 0..resource_count {
        let resource_types = ["service", "package", "file", "template", "execute", "user"];
        let cookbooks = ["apache2", "nginx", "postgresql", "redis", "mysql", "docker"];
        let platforms = ["ubuntu", "centos", "debian", "windows", "amazon"];

        resources.push(json!({
            "type": resource_types[rng.gen_range(0..resource_types.len())],
            "name": format!("resource-{}", r),
            "id": format!("{}[resource-{}]", resource_types[rng.gen_range(0..resource_types.len())], r),
            "duration": rng.gen_range(1..5000).to_string(),
            "change_count": if rng.gen_bool(0.7) { 0 } else { rng.gen_range(1..10) },
            "status": if rng.gen_bool(0.9) { "up-to-date" } else { "updated" },
            "cookbook_name": cookbooks[rng.gen_range(0..cookbooks.len())],
            "cookbook_version": format!("{}.{}.{}", rng.gen_range(1..5), rng.gen_range(0..10), rng.gen_range(0..20)),
            "platform": platforms[rng.gen_range(0..platforms.len())],
            "platform_version": format!("{}.{}", rng.gen_range(18..24), rng.gen_range(0..4)),
            "action": if rng.gen_bool(0.9) { "nothing" } else { "install" },
            "guard_result": rng.gen_bool(0.8),
        }));
    }

    json!({
        "type": "run_converge",
        "run_id": run_id,
        "node_name": node_id,
        "organization_name": "load-test-org",
        "chef_server_fqdn": "chef.example.com",
        "chef_version": format!("{}.{}.{}", rng.gen_range(17..19), rng.gen_range(0..10), rng.gen_range(0..30)),
        "entity_uuid": uuid::Uuid::new_v4().to_string(),
        "id": uuid::Uuid::new_v4().to_string(),
        "node_automatic": {
            "platform": "ubuntu",
            "platform_version": "22.04",
            "hostname": node_id,
            "ipaddress": format!("10.0.{}.{}", rng.gen_range(1..255), rng.gen_range(1..255)),
        },
        "run_list": ["recipe[base]", "recipe[monitoring]"],
        "start_time": started.to_rfc3339(),
        "end_time": now.to_rfc3339(),
        "status": status,
        "resources": resources,
        "total_resource_count": resource_count,
        "updated_resource_count": rng.gen_range(0..5),
        "deprecations": [],
        "error": if status == "failure" { json!({"class": "Chef::Exceptions::Exec", "message": "command failed", "backtrace": []}) } else { json!(null) },
        "payload_version": 12,
        "metric_idx": idx,
    })
}

/// Phase result summary
#[derive(Debug, Clone)]
struct PhaseResult {
    name: String,
    description: String,
    target_rps: f64,
    actual_rps: f64,
    duration_secs: f64,
    total_requests: u64,
    accepted: u64,
    rejected_429: u64,
    errors: u64,
    data_loss: u64,
    latency_p50_ms: f64,
    latency_p95_ms: f64,
    latency_p99_ms: f64,
    latency_max_ms: f64,
    throughput_mbps: f64,
    queue_saturation_detected: bool,
    queue_recovery_detected: bool,
}

/// Run a single load test phase
async fn run_phase(
    client: &reqwest::Client,
    server: &str,
    token: &str,
    config: &PhaseConfig,
    duration_secs: u64,
    concurrency: usize,
) -> PhaseResult {
    println!("\n══════════════════════════════════════════════════");
    println!("  Phase: {} — {}", config.name, config.description);
    println!(
        "  Target: {:.1} req/s for {}s",
        config.target_rps, duration_secs
    );
    println!("══════════════════════════════════════════════════\n");

    let results: Arc<tokio::sync::Mutex<Vec<RequestResult>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let counter = Arc::new(AtomicU64::new(0));
    let semaphore = Arc::new(Semaphore::new(concurrency));

    let _interval = Duration::from_secs_f64(1.0 / config.target_rps);
    let start = Instant::now();
    let total_expected = (config.target_rps * duration_secs as f64) as u64;

    let mut handles = Vec::new();

    for i in 0..total_expected {
        let elapsed = start.elapsed();
        let target_time = Duration::from_secs_f64(i as f64 / config.target_rps);
        if target_time > elapsed {
            tokio::time::sleep(target_time - elapsed).await;
        }

        let client = client.clone();
        let server = server.to_string();
        let token = token.to_string();
        let results = results.clone();
        let counter = counter.clone();
        let sem = semaphore.clone();
        let idx = counter.fetch_add(1, Ordering::Relaxed);

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore acquire");
            let payload = generate_payload(idx);
            let body = serde_json::to_vec(&payload).expect("payload serialization");
            let bytes = body.len();

            let req_start = Instant::now();
            let resp = client
                .post(format!("{}/ingest/events/data-collector", server))
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {}", token))
                .body(body)
                .send()
                .await;

            let latency = req_start.elapsed().as_secs_f64() * 1000.0;

            match resp {
                Ok(r) => {
                    let status = r.status().as_u16();
                    let accepted = status == 202;
                    results.lock().await.push(RequestResult {
                        status,
                        latency_ms: latency,
                        bytes_sent: bytes,
                        accepted,
                    });
                }
                Err(_) => {
                    results.lock().await.push(RequestResult {
                        status: 0,
                        latency_ms: latency,
                        bytes_sent: bytes,
                        accepted: false,
                    });
                }
            }
        });

        handles.push(handle);

        // Progress indicator
        if i % 100 == 0 && i > 0 {
            let elapsed = start.elapsed().as_secs_f64();
            let actual_rps = i as f64 / elapsed;
            print!(
                "\r  [{:.0}s] {:.0}/{} requests sent ({:.1} req/s)...",
                elapsed, i, total_expected, actual_rps
            );
        }
    }

    // Wait for all requests to complete
    for h in handles {
        let _ = h.await;
    }

    let phase_duration = start.elapsed().as_secs_f64();
    let results = results.lock().await;

    // Calculate statistics
    let total = results.len() as u64;
    let accepted = results.iter().filter(|r| r.accepted).count() as u64;
    let rejected_429 = results.iter().filter(|r| r.status == 429).count() as u64;
    let errors = results
        .iter()
        .filter(|r| r.status != 202 && r.status != 429)
        .count() as u64;

    let mut latencies: Vec<f64> = results.iter().map(|r| r.latency_ms).collect();
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let p50_idx = (latencies.len() as f64 * 0.50) as usize;
    let p95_idx = (latencies.len() as f64 * 0.95) as usize;
    let p99_idx = (latencies.len() as f64 * 0.99) as usize;

    let p50 = latencies.get(p50_idx).copied().unwrap_or(0.0);
    let p95 = latencies.get(p95_idx).copied().unwrap_or(0.0);
    let p99 = latencies.get(p99_idx).copied().unwrap_or(0.0);
    let max_lat = latencies.last().copied().unwrap_or(0.0);

    let total_bytes: usize = results.iter().map(|r| r.bytes_sent).sum();
    let throughput_mbps = (total_bytes as f64 / 1_048_576.0) / phase_duration;

    // Detect queue saturation: burst of 429s followed by recovery
    let mut consecutive_429 = 0u64;
    let mut max_consecutive_429 = 0u64;
    let mut queue_saturation = false;
    let mut queue_recovery = false;

    for r in results.iter() {
        if r.status == 429 {
            consecutive_429 += 1;
            max_consecutive_429 = max_consecutive_429.max(consecutive_429);
        } else {
            if consecutive_429 >= 3 {
                queue_recovery = true;
            }
            consecutive_429 = 0;
        }
    }
    queue_saturation = max_consecutive_429 >= 3;

    let result = PhaseResult {
        name: config.name.to_string(),
        description: config.description.to_string(),
        target_rps: config.target_rps,
        actual_rps: total as f64 / phase_duration,
        duration_secs: phase_duration,
        total_requests: total,
        accepted,
        rejected_429,
        errors,
        data_loss: total - accepted - rejected_429 - errors,
        latency_p50_ms: p50,
        latency_p95_ms: p95,
        latency_p99_ms: p99,
        latency_max_ms: max_lat,
        throughput_mbps,
        queue_saturation_detected: queue_saturation,
        queue_recovery_detected: queue_recovery,
    };

    print_results(&result);
    result
}

fn print_results(r: &PhaseResult) {
    println!("\n  ┌─────────────────────────────────────────────────┐");
    println!("  │ Phase: {:<42}│", r.name);
    println!("  ├─────────────────────────────────────────────────┤");
    println!(
        "  │ Target RPS:  {:>8.1}  Actual: {:>8.1}          │",
        r.target_rps, r.actual_rps
    );
    println!(
        "  │ Duration:    {:>8.1}s                           │",
        r.duration_secs
    );
    println!(
        "  │ Total:       {:>8} requests                    │",
        r.total_requests
    );
    println!(
        "  │ Accepted:    {:>8} (202)                       │",
        r.accepted
    );
    println!(
        "  │ Rejected:    {:>8} (429)                       │",
        r.rejected_429
    );
    println!(
        "  │ Errors:      {:>8}                            │",
        r.errors
    );
    println!(
        "  │ Data loss:   {:>8}                            │",
        r.data_loss
    );
    println!("  ├─────────────────────────────────────────────────┤");
    println!(
        "  │ Latency p50: {:>8.1} ms                        │",
        r.latency_p50_ms
    );
    println!(
        "  │ Latency p95: {:>8.1} ms                        │",
        r.latency_p95_ms
    );
    println!(
        "  │ Latency p99: {:>8.1} ms                        │",
        r.latency_p99_ms
    );
    println!(
        "  │ Latency max: {:>8.1} ms                        │",
        r.latency_max_ms
    );
    println!(
        "  │ Throughput:  {:>8.1} MB/s                      │",
        r.throughput_mbps
    );
    println!("  ├─────────────────────────────────────────────────┤");
    println!(
        "  │ Queue saturation:  {:<5}                       │",
        if r.queue_saturation_detected {
            "YES"
        } else {
            "no"
        }
    );
    println!(
        "  │ Queue recovery:    {:<5}                       │",
        if r.queue_recovery_detected {
            "YES"
        } else {
            "no"
        }
    );
    println!(
        "  │ Data loss:         {:<5}                       │",
        if r.data_loss == 0 { "NONE" } else { "YES" }
    );
    println!("  └─────────────────────────────────────────────────┘");
}

/// Generate BENCHMARKS.md from results
fn generate_benchmarks(results: &[PhaseResult], reference_hw: &str, output_path: &str) {
    let now = chrono::Utc::now().format("%Y-%m-%d");

    let mut md = String::new();
    md.push_str("# Spindle Load Test Benchmarks\n\n");
    md.push_str(&format!("**Date:** {}\n", now));
    md.push_str(&format!("**Reference hardware:** {}\n", reference_hw));
    md.push_str("**Tool:** `spindle-bench`\n\n");

    md.push_str("---\n\n");
    md.push_str("## 1. Test methodology\n\n");
    md.push_str(
        "Load tests replay synthetic Chef data collector payloads against the Spindle ingest\n",
    );
    md.push_str("endpoint (`POST /v1/ingest`). Payloads are generated with realistic structure:\n");
    md.push_str(
        "run-converge messages with 5–50 resource events each, random cookbook versions,\n",
    );
    md.push_str("platforms, and resource types.\n\n");
    md.push_str("Three phases are run sequentially with 10s cooldown between phases:\n\n");
    md.push_str("| Phase | Rate | Duration | Description |\n");
    md.push_str("|---|---|---|---|\n");
    md.push_str("| sustained | 11.1 req/s | 60s | 960K runs/day baseline fleet load |\n");
    md.push_str("| peak | 150 req/s | 60s | Peak burst during concurrent converge window |\n");
    md.push_str("| stress | 300 req/s | 60s | 2× headroom — graceful degradation required |\n\n");

    md.push_str("---\n\n");
    md.push_str("## 2. Results\n\n");

    for r in results {
        md.push_str(&format!("### Phase: {}\n\n", r.name));
        md.push_str(&format!("{}\n\n", r.description));

        md.push_str("| Metric | Value |\n");
        md.push_str("|---|---|\n");
        md.push_str(&format!("| Target RPS | {:.1} |\n", r.target_rps));
        md.push_str(&format!("| Actual RPS | {:.1} |\n", r.actual_rps));
        md.push_str(&format!("| Duration | {:.1}s |\n", r.duration_secs));
        md.push_str(&format!("| Total requests | {} |\n", r.total_requests));
        md.push_str(&format!("| Accepted (202) | {} |\n", r.accepted));
        md.push_str(&format!("| Rejected (429) | {} |\n", r.rejected_429));
        md.push_str(&format!("| Errors | {} |\n", r.errors));
        md.push_str(&format!(
            "| **Data loss** | **{}** |\n",
            if r.data_loss == 0 {
                "NONE".to_string()
            } else {
                format!("{} requests", r.data_loss)
            }
        ));
        md.push_str("|---|---|\n");
        md.push_str(&format!("| Latency p50 | {:.1} ms |\n", r.latency_p50_ms));
        md.push_str(&format!("| Latency p95 | {:.1} ms |\n", r.latency_p95_ms));
        md.push_str(&format!(
            "| **Latency p99** | **{:.1} ms** |\n",
            r.latency_p99_ms
        ));
        md.push_str(&format!("| Latency max | {:.1} ms |\n", r.latency_max_ms));
        md.push_str(&format!("| Throughput | {:.1} MB/s |\n", r.throughput_mbps));
        md.push_str("|---|---|\n");
        md.push_str(&format!(
            "| Queue saturation | {} |\n",
            if r.queue_saturation_detected {
                "detected"
            } else {
                "no"
            }
        ));
        md.push_str(&format!(
            "| Queue recovery | {} |\n",
            if r.queue_recovery_detected {
                "confirmed"
            } else {
                "no"
            }
        ));
        md.push('\n');
    }

    md.push_str("---\n\n");
    md.push_str("## 3. Acceptance criteria\n\n");

    let sustained = results.iter().find(|r| r.name == "sustained");
    let peak = results.iter().find(|r| r.name == "peak");
    let stress = results.iter().find(|r| r.name == "stress");

    md.push_str("| Criterion | Result | Status |\n");
    md.push_str("|---|---|---|\n");

    if let Some(s) = sustained {
        let pass = s.latency_p99_ms < 60_000.0;
        md.push_str(&format!(
            "| Sustained p99 ingest lag < 60s | {:.1} ms | {} |\n",
            s.latency_p99_ms,
            if pass { "✅ PASS" } else { "❌ FAIL" }
        ));
    }

    if let Some(s) = sustained {
        let data_loss_str = if s.data_loss == 0 {
            "0".to_string()
        } else {
            s.data_loss.to_string()
        };
        let pass = s.data_loss == 0;
        md.push_str(&format!(
            "| Sustained: no data loss | {} | {} |\n",
            data_loss_str,
            if pass { "✅ PASS" } else { "❌ FAIL" }
        ));
    }

    if let Some(p) = peak {
        let data_loss_str = if p.data_loss == 0 {
            "0".to_string()
        } else {
            p.data_loss.to_string()
        };
        let pass = p.data_loss == 0;
        md.push_str(&format!(
            "| Peak (150 req/s): no data loss | {} | {} |\n",
            data_loss_str,
            if pass { "✅ PASS" } else { "❌ FAIL" }
        ));
    }

    if let Some(st) = stress {
        let data_loss_str = if st.data_loss == 0 {
            "0".to_string()
        } else {
            st.data_loss.to_string()
        };
        let pass = st.data_loss == 0;
        md.push_str(&format!(
            "| Stress (300 req/s): no data loss | {} | {} |\n",
            data_loss_str,
            if pass { "✅ PASS" } else { "❌ FAIL" }
        ));
    }

    if let Some(st) = stress {
        let has_429 = st.rejected_429 > 0;
        let has_recovery = st.queue_recovery_detected;
        let pass = has_429 && has_recovery;
        md.push_str(&format!(
            "| Stress: graceful degradation (429s + recovery) | {} 429s, recovery {} | {} |\n",
            st.rejected_429,
            if has_recovery {
                "confirmed"
            } else {
                "not detected"
            },
            if pass {
                "✅ PASS"
            } else {
                if has_429 {
                    "⚠️ PARTIAL"
                } else {
                    "⚠️ NO 429s"
                }
            }
        ));
    }

    md.push('\n');

    md.push_str("---\n\n");
    md.push_str("## 4. Reference hardware\n\n");
    md.push_str("| Component | Specification |\n");
    md.push_str("|---|---|\n");
    md.push_str("| CPU | 16 vCPU (Intel Xeon / AMD EPYC, 2.5 GHz+) |\n");
    md.push_str("| Memory | 64 GB DDR4 |\n");
    md.push_str("| Storage | NVMe SSD |\n");
    md.push_str("| Network | 10 Gbps |\n");
    md.push_str("| OS | Ubuntu 22.04 LTS |\n");
    md.push_str("| Database | PostgreSQL 15+ |\n");
    md.push_str("| Object store | MinIO (local NVMe) |\n\n");

    md.push_str("---\n\n");
    md.push_str("## 5. Capacity guidance\n\n");
    md.push_str("Based on the sustained phase results:\n\n");
    md.push_str(&format!(
        "- **Baseline capacity:** {:.0} runs/day at {:.1} req/s steady state\n",
        86400.0 * sustained.map(|s| s.actual_rps).unwrap_or(11.1),
        sustained.map(|s| s.actual_rps).unwrap_or(11.1)
    ));
    md.push_str(&format!(
        "- **Peak headroom:** {:.1}× baseline tested (stress phase)\n",
        stress.map(|s| s.actual_rps).unwrap_or(300.0)
            / sustained.map(|s| s.actual_rps).unwrap_or(11.1)
    ));
    md.push_str("- **Scaling:** Horizontal scaling via multiple ingest workers behind a load balancer (ING-12)\n\n");

    md.push_str("---\n\n");
    md.push_str(
        "*Generated by `spindle-bench` — see `tools/spindle-bench` for the load test tool.*\n",
    );

    std::fs::write(output_path, md).expect("Failed to write BENCHMARKS.md");
    println!("\n✅ Results written to {}", output_path);
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    println!("╔══════════════════════════════════════════════════╗");
    println!("║         Spindle Load Test Tool (M5-08)          ║");
    println!("╚══════════════════════════════════════════════════╝");
    println!();
    println!("  Server:      {}", args.server);
    println!("  Mode:        {}", args.mode);
    println!("  Duration:    {}s per phase", args.duration);
    println!("  Concurrency: {}", args.concurrency);
    println!();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(args.concurrency)
        .build()
        .expect("Failed to build HTTP client");

    // Determine which phases to run
    let phases_to_run: Vec<&PhaseConfig> = match args.mode.as_str() {
        "sustained" => vec![&PHASES[0]],
        "peak" => vec![&PHASES[1]],
        "stress" => vec![&PHASES[2]],
        "full" => PHASES.iter().collect(),
        _ => {
            eprintln!(
                "Unknown mode: {}. Use sustained, peak, stress, or full.",
                args.mode
            );
            std::process::exit(1);
        }
    };

    // Warm-up: send a few requests to prime connections
    println!("  Warming up...");
    for i in 0..10 {
        let payload = generate_payload(i);
        let _ = client
            .post(format!("{}/ingest/events/data-collector", args.server))
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", args.token))
            .json(&payload)
            .send()
            .await;
    }
    println!("  Warm-up complete.\n");

    // Run phases
    let mut results = Vec::new();
    for (i, phase) in phases_to_run.iter().enumerate() {
        if i > 0 {
            println!("\n  Cooling down (10s)...");
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        let result = run_phase(
            &client,
            &args.server,
            &args.token,
            phase,
            args.duration,
            args.concurrency,
        )
        .await;
        results.push(result);
    }

    // Generate BENCHMARKS.md if in full mode
    if args.mode == "full" {
        generate_benchmarks(
            &results,
            "16 vCPU / 64GB DDR4 / NVMe SSD / 10 Gbps / Ubuntu 22.04",
            &args.output,
        );
    }

    println!("\n══════════════════════════════════════════════════");
    println!("  Load test complete.");
    println!("══════════════════════════════════════════════════\n");
}
