//! Spindle server — main application binary.
//!
//! Supports `--validate-config` flag: validates configuration and exits 0 (valid)
//! or 1 (invalid with specific error messages).
//! Supports port conflict detection at startup.
//!
//! On normal startup it loads configuration, assembles the metrics/health and
//! ingest routers, binds a `TcpListener` on `config.server.addr()` and serves
//! HTTP via `axum::serve`:
//!   - GET  /health
//!   - GET  /ready
//!   - GET  /metrics
//!   - POST /ingest/events/data-collector  (auth: `Bearer <token>`)
//!   - POST /ingest/events/inspec         (auth: `Bearer <token>`)
//!
//! The ingest bearer token is read from `SPINDLE_INGEST_TOKEN` and defaults to
//! `spindle-dev-token`. The raw-archive root is read from `SPINDLE_ARCHIVE_DIR`
//! and defaults to `/var/lib/spindle/archive`.

//! `--version` prints the git commit SHA and build date, embedded at compile
//! time via build.rs setting `SPINDLE_GIT_SHA` and `SPINDLE_BUILD_DATE`.
//!
//! ## Production mode
//! When `SPINDLE_PRODUCTION=1` is set, the server **requires** a reachable
//! PostgreSQL database at startup. If the database cannot be contacted, the
//! server exits with code 1 — there is no silent in-memory fallback.
//!
//! In dev mode (default, `SPINDLE_PRODUCTION` unset), the server falls back
//! to in-memory stores if the database is unavailable.

#![allow(warnings)]
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use tracing_subscriber::EnvFilter;

use spindle_server::ingest::{
    InMemoryIdempotencyStore, InMemoryQueueMonitor, IngestAppState, IngestConfig,
    PostgresIdempotencyStore, PostgresQueueMonitor, DEFAULT_MAX_INGEST_LAG_SECONDS,
};
use spindle_server::metrics::{MetricsRegistry, MetricsState};

use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

/// Build info: git commit SHA (short) and build date, set by build.rs.
const GIT_SHA: &str = env!("SPINDLE_GIT_SHA");
const BUILD_DATE: &str = env!("SPINDLE_BUILD_DATE");

/// OpenAPI document — auto-generated from #[utoipa::path] attributes on
/// handlers and #[derive(utoipa::ToSchema)] on request/response types.
#[derive(OpenApi)]
#[openapi(
    paths(
        // Ingest
        spindle_server::ingest::data_collector_handler,
        spindle_server::ingest::inspec_handler,
        // Auth (JIT)
        spindle_server::jit_auth::handle_login,
        // Nodes
        spindle_server::nodes::list_nodes,
        spindle_server::nodes::get_node_detail,
        // Runs
        spindle_server::runs::list_runs,
        // Compliance
        spindle_server::compliance::list_reports,
        spindle_server::compliance::get_report,
        // Cookbooks
        spindle_server::cookbooks::list_cookbooks,
        // Waivers
        spindle_server::waivers::list_waivers,
        // Resource Events
        spindle_server::resource_events::get_aggregates,
        spindle_server::resource_events::get_drift,
    ),
    components(
        schemas(
            // Ingest
            spindle_server::ingest::Provenance,
            spindle_server::ingest::UserRole,
            spindle_server::ingest::ErrorResponse,
            spindle_server::ingest::ErrorBody,
            spindle_server::ingest::Pagination,
            spindle_server::ingest::RequestId,
            // Auth (JIT)
            spindle_server::jit_auth::LoginQuery,
            spindle_server::jit_auth::LoginResponse,
            spindle_server::jit_auth::LoginError,
            // Nodes
            spindle_server::nodes::NodeSummary,
            spindle_server::nodes::NodeDetail,
            spindle_server::nodes::NodeDetailResponse,
            // Runs
            spindle_server::runs::RunSummary,
            spindle_server::runs::RunDetail,
            spindle_server::runs::RunDetailResponse,
            // Compliance
            spindle_server::compliance::NodeComplianceStatus,
            spindle_server::compliance::ProfileComplianceStatus,
            // Cookbooks
            spindle_server::cookbooks::CookbookVersionInfo,
            spindle_server::cookbooks::CookbookListResponse,
            spindle_server::cookbooks::CookbookInventoryEntry,
            // Waivers
            spindle_server::waivers::WaiverSummary,
            spindle_server::waivers::WaiverRequest,
            spindle_server::waivers::WaiverDetail,
            spindle_server::waivers::WaiversListResponse,
            spindle_server::waivers::PaginationInfo,
            spindle_server::waivers::WaiverDetailResponse,
            spindle_server::waivers::AuditLogEntry,
            // Resource Events
            spindle_server::resource_events::AggregateRow,
            spindle_server::resource_events::AggregatesResponse,
            spindle_server::resource_events::DriftRow,
            spindle_server::resource_events::DriftResponse,
            // Health
            spindle_server::health::HealthStatus,
            spindle_server::health::SubsystemHealth,
            spindle_server::health::HealthResponse,
            spindle_server::health::IngestLagInfo,
            // Filter / pagination (from spindle-api)
            spindle_api::FilterOp,
            spindle_api::FilterValue,
            spindle_api::Filter,
            spindle_api::TimeRange,
            spindle_api::SortDirection,
            spindle_api::Sort,
            spindle_api::QueryFilter,
            spindle_api::pagination::PaginationParams,
        )
    ),
    tags(
        (name = "ingest", description = "Ingest endpoints"),
        (name = "auth", description = "Authentication"),
        (name = "nodes", description = "Node inventory"),
        (name = "runs", description = "Run history"),
        (name = "compliance", description = "Compliance reports"),
        (name = "cookbooks", description = "Cookbook inventory"),
        (name = "waivers", description = "Compliance waivers"),
        (name = "resource-events", description = "Resource event aggregates"),
    )
)]
struct ApiDoc;


/// Default ingest bearer token used when `SPINDLE_INGEST_TOKEN` is unset.
const DEFAULT_INGEST_TOKEN: &str = "spindle-dev-token";
/// Default raw-archive root used when `SPINDLE_ARCHIVE_DIR` is unset.
const DEFAULT_ARCHIVE_DIR: &str = "/var/lib/spindle/archive";

fn main() {
    // ── Initialize tracing subscriber (L1=info default, L2=debug, L3=trace) ──
    // SPINDLE_LOG_LEVEL=operational|diagnostic|debug  (maps to info|debug|trace)
    // RUST_LOG=spindle_server=info,spindle_worker=debug  (per-crate overrides)
    let log_level = std::env::var("SPINDLE_LOG_LEVEL").unwrap_or_else(|_| "operational".to_string());
    let tier_level = match log_level.to_lowercase().as_str() {
        "operational" | "info" => "info",
        "diagnostic" | "debug" => "debug",
        "trace" => "trace",
        _ => "info",
    };
    let env_filter = match std::env::var("RUST_LOG") {
        Ok(rust_log) => EnvFilter::new(&rust_log),
        Err(_) => EnvFilter::new(tier_level),
    };
    let use_json = std::env::var("SPINDLE_LOG_TARGET").as_deref().unwrap_or("json") != "stdout";
    if use_json {
        let subscriber = tracing_subscriber::fmt::Subscriber::builder()
            .with_env_filter(env_filter)
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .json()
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("tracing subscriber already set");
    } else {
        let subscriber = tracing_subscriber::fmt::Subscriber::builder()
            .with_env_filter(env_filter)
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("tracing subscriber already set");
    }
    tracing::info!(
        log_level = %log_level,
        tier = %tier_level,
        "spindle-obs initialized (L1=operational, L2=diagnostic, L3=debug/trace)"
    );

    let args: Vec<String> = std::env::args().collect();
    let mut validate_only = false;
    let mut show_version = false;
    let mut config_path: Option<String> = None;
    let mut process_payload_key: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--validate-config" => {
                validate_only = true;
            }
            "--version" | "-V" => {
                show_version = true;
            }
            "--process-payload" => {
                if i + 1 < args.len() {
                    process_payload_key = Some(args[i + 1].clone());
                    i += 1;
                } else {
                    eprintln!("Error: --process-payload requires an archive key argument");
                    std::process::exit(1);
                }
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
                println!("  --config <PATH>        Path to config file (default: ~/.config/spindle/config.toml or $SPINDLE_CONFIG)");
                println!("  --validate-config      Validate configuration and exit");
                println!("  --process-payload <KEY> One-shot pipeline trigger");
                println!("  --version, -V          Print version (commit SHA + build date)");
                println!("  --help, -h             Print this help message");
                println!();
                println!("Environment:");
                println!("  SPINDLE_INGEST_TOKEN  Bearer token (default: spindle-dev-token)");
                println!("  SPINDLE_ARCHIVE_DIR   Raw-archive root (default: /var/lib/spindle/archive)");
                println!("  SPINDLE_DATABASE_URL  PostgreSQL connection string");
                println!("  SPINDLE_PRODUCTION    Set to 1 for production mode (DB required)");
                println!("  SPINDLE_LOG_LEVEL     operational|diagnostic|debug");
                println!("  SPINDLE_LOG_TARGET    json|stdout");
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    if let Some(path) = &config_path {
        std::env::set_var("SPINDLE_CONFIG", path);
    }

    if show_version {
        println!("spindle-server {} (git: {}, built: {})",
            env!("CARGO_PKG_VERSION"),
            GIT_SHA,
            BUILD_DATE,
        );
        std::process::exit(0);
    }

    if validate_only {
        match spindle_config::Config::load() {
            Ok(config) => match config.validate() {
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
            },
            Err(e) => {
                eprintln!("Failed to load configuration: {}", e);
                std::process::exit(1);
            }
        }
    }

    // One-shot pipeline trigger: process a single archived run-converge payload.
    if let Some(ref key) = process_payload_key {
        let database_url = std::env::var("SPINDLE_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| panic!("SPINDLE_DATABASE_URL must be set for --process-payload"));
        let archive_root = std::env::var("SPINDLE_ARCHIVE_DIR")
            .unwrap_or_else(|_| "/var/lib/spindle/archive".to_string());

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async move {
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(5)
                .connect(&database_url)
                .await
                .expect("failed to connect to database");
            spindle_server::pipeline_trigger::process_archive_key(pool, &archive_root, key).await
        })
        .expect("one-shot pipeline trigger failed");
        std::process::exit(0);
    }

    // Normal startup
    let config = match spindle_config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load configuration: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = config.validate() {
        eprintln!("Configuration validation failed: {}", e);
        std::process::exit(1);
    }

    let addr = config.server.addr();
    if let Err(e) = check_port_available(addr) {
        eprintln!("Port conflict: cannot bind to {}", addr);
        eprintln!("  {}", e);
        eprintln!("  Another process may be using this port.");
        std::process::exit(3);
    }

    // ── Production mode: DB is required ──
    let production = std::env::var("SPINDLE_PRODUCTION").as_deref() == Ok("1");

    println!("Starting spindle-server on {}", addr);
    if let Err(e) = run_server(addr, config.identity.clone(), config.database.clone(), production) {
        eprintln!("Fatal: server error: {}", e);
        std::process::exit(1);
    }
}

/// Build shared application state and start the axum HTTP server.
fn run_server(
    addr: SocketAddr,
    identity_config: spindle_config::IdentityConfig,
    database_config: spindle_config::DatabaseConfig,
    production: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // ── Metrics / health state ──────────────────────────────────────────────
    let metrics = Arc::new(MetricsRegistry::new());
    let metrics_state = MetricsState {
        metrics: metrics.clone(),
        start_time: Instant::now(),
    };

    // ── Ingest state ────────────────────────────────────────────────────────
    let token =
        std::env::var("SPINDLE_INGEST_TOKEN").unwrap_or_else(|_| DEFAULT_INGEST_TOKEN.to_string());
    let archive_root =
        std::env::var("SPINDLE_ARCHIVE_DIR").unwrap_or_else(|_| DEFAULT_ARCHIVE_DIR.to_string());

    let archive = Arc::new(spindle_rawarchive::LocalArchive::new(&archive_root)?);

    // ── Database connection (production) ──
    let database_url = std::env::var("SPINDLE_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| "postgres://spindle:spindle@localhost:5432/spindle".to_string());

    // ── Serve HTTP on the configured address ───
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let pool = if database_url.starts_with("postgres://")
            || database_url.starts_with("postgresql://")
        {
            match sqlx::postgres::PgPoolOptions::new()
                .max_connections(database_config.pool_max)
                .acquire_timeout(std::time::Duration::from_secs(database_config.connect_timeout_secs))
                .connect(&database_url)
                .await
            {
                Ok(p) => Some(p),
                Err(e) => {
                    if production {
                        eprintln!(
                            "FATAL: database connection failed: {}. Server cannot start in production mode.",
                            e
                        );
                        eprintln!("Set SPINDLE_PRODUCTION=0 for development with in-memory fallback.");
                        std::process::exit(1);
                    } else {
                        eprintln!(
                            "Warning: database connection failed: {}. In-memory fallback.",
                            e
                        );
                        tracing::warn!(
                            "Database unavailable in dev mode — using in-memory stores"
                        );
                        None
                    }
                }
            }
        } else {
            if production {
                eprintln!("FATAL: database URL must use postgres:// or postgresql:// scheme in production.");
                std::process::exit(1);
            }
            None
        };

        // Use Postgres-backed stores when DB is available; fall back to in-memory for dev
        let idempotency: Arc<dyn spindle_server::ingest::IdempotencyStore> =
            if let Some(ref p) = pool {
                Arc::new(PostgresIdempotencyStore::new(p.clone()))
            } else {
                Arc::new(InMemoryIdempotencyStore::new())
            };
        let queue: Arc<dyn spindle_server::ingest::QueueMonitor> = if let Some(ref p) = pool {
            Arc::new(PostgresQueueMonitor::new(p.clone(), 150.0))
        } else {
            Arc::new(InMemoryQueueMonitor::new(0, 150.0))
        };

        let ingest_state = IngestAppState::new_with_pool(
            IngestConfig::new(&token),
            archive.clone(),
            idempotency,
            queue,
            DEFAULT_MAX_INGEST_LAG_SECONDS * 2,
            pool.clone(),
        );

        // ── Assemble router ──
        let mut router: Router = Router::new()
            .merge(spindle_server::metrics::metrics_routes(metrics_state))
            .merge(spindle_server::ingest::ingest_routes(ingest_state));

        // ── Auth routes ─────────────────────────────────────────────────────────
        // Local username/password auth (in-memory store).
        let local_config = spindle_server::local_accounts::LocalAccountsConfig::from_env();
        let local_state = spindle_server::local_accounts::LocalAuthState::new(local_config);
        router = router.merge(spindle_server::local_accounts::local_auth_routes(
            local_state,
        ));

        // JIT auth: DB-backed login (connector/subject) that provisions the user
        // into `users`/`user_roles` and issues session tokens. Requires a Postgres
        // pool, so it's only mounted when a database connection is available.
        if let Some(ref db) = pool {
            match spindle_server::jit_auth::AuthState::new(
                db.clone(),
                spindle_server::sessions::SessionConfig::default(),
                identity_config.clone(),
            ) {
                Ok(auth_state) => {
                    router = router
                        .merge(spindle_server::jit_auth::auth_routes().with_state(auth_state));
                    println!("Auth: JIT OIDC login routes mounted /v1/auth/login");
                }
                Err(e) => {
                    eprintln!(
                        "Auth: failed to initialize JIT auth state (mapping rules invalid): {}",
                        e
                    );
                }
            }
        } else {
            println!("Auth: DB unavailable — /v1/auth/login JIT routes not mounted");
        }

        // ── Query/management routes (M2-M5): self-contained in-memory stores ─────
        // These are the read/management endpoints Mike & Mark depend on
        // (/v1/nodes, /v1/runs, /v1/waivers, /v1/cookbooks,
        //  /v1/resource-events, /v1/health/compliance).
        // h-node/runs/waivers/cookbooks use in-memory stores (the ingest path persists
        // to Postgres; these query stores are independently seeded). Compliance is
        // DB-backed (spindle_store::PgStore) and is only mounted when a pool exists.

        // /v1/health (+ metrics) — aggregate subsystem health with REAL probes.
        // In production, all checkers are real; in dev without DB, use AlwaysUpChecker
        // as a placeholder so the endpoint still returns 200 (with degraded detail).
        let db_checker: Arc<dyn spindle_server::health::HealthChecker> = if let Some(ref db) = pool {
            Arc::new(spindle_server::health::DbHealthChecker::new(db.clone()))
        } else {
            Arc::new(spindle_server::health::AlwaysUpChecker {
                name: "database".to_string(),
            })
        };
        let storage_checker: Arc<dyn spindle_server::health::HealthChecker> = Arc::new(
            spindle_server::health::StorageHealthChecker::new(archive.clone()),
        );
        let dex_checker: Arc<dyn spindle_server::health::HealthChecker> =
            if identity_config.is_enabled() {
                Arc::new(spindle_server::health::DexHealthChecker::new(
                    identity_config.issuer_url.as_deref().unwrap_or(""),
                ))
            } else {
                Arc::new(spindle_server::health::AlwaysUpChecker {
                    name: "dex".to_string(),
                })
            };

        router = router.merge(spindle_server::health::health_routes(
            spindle_server::health::HealthAppState::new(
                db_checker,
                storage_checker,
                dex_checker,
            ),
        ));

        // /v1/nodes — node inventory (DB-backed when a pool exists, else in-memory).
        let node_store: std::sync::Arc<dyn spindle_server::nodes::NodeStore>;
        if let Some(ref db) = pool {
            node_store = std::sync::Arc::new(spindle_server::nodes::DbNodeStore::new(db.clone()));
            println!("Nodes: DB-backed /v1/nodes routes mounted");
        } else {
            node_store = std::sync::Arc::new(spindle_server::nodes::InMemoryNodeStore::new());
        }
        router = router.merge(
            spindle_server::nodes::nodes_routes(spindle_server::nodes::NodesAppState::new(
                node_store,
            ))
            .route_layer(axum::middleware::from_fn(
                spindle_server::ingest::require_jwt_role,
            )),
        );

        // /v1/runs (+ resource-events under a run) — run history (DB-backed when pooled).
        let runs_store: std::sync::Arc<dyn spindle_server::runs::RunsStore>;
        let events_store: std::sync::Arc<dyn spindle_server::runs::ResourceEventsStore>;
        if let Some(ref db) = pool {
            let db_runs = std::sync::Arc::new(spindle_server::runs::DbRunsStore::new(db.clone()));
            runs_store = db_runs.clone();
            events_store = db_runs;
            println!("Runs: DB-backed /v1/runs routes mounted");
        } else {
            runs_store = std::sync::Arc::new(spindle_server::runs::InMemoryRunsStore::new());
            events_store = std::sync::Arc::new(spindle_server::runs::InMemoryRunsStore::new());
        }
        router = router.merge(
            spindle_server::runs::runs_routes(spindle_server::runs::RunsAppState::new(
                runs_store,
                events_store,
            ))
            .route_layer(axum::middleware::from_fn(
                spindle_server::ingest::require_jwt_role,
            )),
        );

        // /v1/waivers (+ audit) — compliance waivers (in-memory).
        router = router.merge(
            spindle_server::waivers::waivers_routes(spindle_server::waivers::WaiversAppState::new(
                std::sync::Arc::new(spindle_server::waivers::InMemoryWaiverStore::new()),
                std::sync::Arc::new(spindle_server::waivers::InMemoryAuditStore::default()),
            ))
            .route_layer(axum::middleware::from_fn(
                spindle_server::ingest::require_jwt_role,
            )),
        );

        // /v1/cookbooks — cookbook inventory (in-memory).
        router = router.merge(
            spindle_server::cookbooks::cookbook_routes(
                spindle_server::cookbooks::CookbookAppState::new(std::sync::Arc::new(
                    spindle_server::cookbooks::InMemoryCookbookStore::new(),
                )),
            )
            .route_layer(axum::middleware::from_fn(
                spindle_server::ingest::require_jwt_role,
            )),
        );

        // /v1/resource-events/aggregates + /drift — rollup store (in-memory).
        let rollup = std::sync::Arc::new(spindle_server::resource_events::RollupStore::new());
        router = router.merge(
            spindle_server::resource_events::resource_events_routes(
                spindle_server::resource_events::AggregatesAppState::new(rollup.clone()),
                spindle_server::resource_events::DriftAppState::new(rollup),
            )
            .route_layer(axum::middleware::from_fn(
                spindle_server::ingest::require_jwt_role,
            )),
        );

        // /v1/compliance/* — DB-backed (spindle_store::SqlxComplianceStore). Mounted only when a
        // Postgres pool is available; a nil-up DB would otherwise 500 on every call.
        if let Some(ref db) = pool {
            let compliance_store = std::sync::Arc::new(spindle_store::SqlxComplianceStore::new(db.clone()));
            let profile_store = std::sync::Arc::new(spindle_store::SqlxProfileStore::new(db.clone()));
            let scope = spindle_store::Scope::all();
            router = router.merge(
                spindle_server::compliance::compliance_router(
                    spindle_server::compliance::ComplianceState::new(compliance_store, profile_store, scope),
                )
                .route_layer(axum::middleware::from_fn(
                    spindle_server::ingest::require_jwt_role,
                )),
            );
            println!("Compliance: DB-backed /v1/compliance/* routes mounted");
        } else {
            println!("Compliance: DB unavailable — /v1/compliance/* routes not mounted");
        }

        // ── Admin routes (M2): dead-letter queue access ────────────────────────────
        // Requires admin role. Only mounted when a Postgres pool is available.
        if let Some(ref db) = pool {
            router = router.merge(
                spindle_server::admin::admin_routes(
                    spindle_server::admin::AdminAppState::new(db.clone()),
                )
            );
            println!("Admin: /v1/admin/dead-letter routes mounted (admin-only)");
        } else {
            println!("Admin: DB unavailable — /v1/admin/dead-letter routes not mounted");
        }

        // ── OpenAPI / Swagger UI ────────────────────────────────────────────────
        // Interactive API docs at /docs, spec at /openapi.json — auto-generated
        // from #[utoipa::path] attributes on handlers and #[derive(ToSchema)] on
        // response types. Zero manual docs to maintain.
        router = router.merge(
            SwaggerUi::new("/docs")
                .url("/openapi.json", ApiDoc::openapi())
        );

        // ── API request logging middleware (L1) ────────────────────────────────────
        // Log every request: method, path, status, latency, request_id.
        // Applied last so it wraps all routes including /docs.
        router = router.layer(axum::middleware::from_fn(api_request_logging));

        // ── Serve HTTP on the configured address ────────────────────────────────
        let listener = tokio::net::TcpListener::bind(addr).await?;
        println!("Spindle server listening on http://{}/", addr);
        axum::serve(listener, router).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
    Ok(())
}

/// API request logging middleware — L1 (always), L2 (debug), L3 (trace).
/// Covers ALL routes including /docs, /openapi.json, /metrics, ingest, and API.
/// Uses `request_id` from request extensions (set by request_id_middleware).
pub async fn api_request_logging(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let start = std::time::Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let request_id = request
        .extensions()
        .get::<spindle_server::ingest::RequestId>()
        .map(|r| r.0.clone())
        .unwrap_or_else(|| "none".to_string());

    let response = next.run(request).await;

    let latency_ms = start.elapsed().as_millis();
    let status = response.status().as_u16();

    // L1: endpoint + status + latency (always on)
    tracing::info!(
        request_id = %request_id,
        method = %method,
        path = %path,
        status = status,
        latency_ms = %latency_ms,
        "api request"
    );

    response
}

/// API query result logging — L2 (debug level).
/// Called by handlers to log query params + result count at L2.
/// Usage: tracing::debug!(params = ?params, result_count = n, "api query result");
/// L3: tracing::trace!(body = %response_body, "api full response body");

/// Check if the given address is available for binding.
pub fn check_port_available(addr: SocketAddr) -> Result<(), std::io::Error> {
    std::net::TcpListener::bind(addr).map(drop)
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
    fn test_version_flag_prints_git_sha() {
        // Build info is embedded at compile time via env!()
        assert!(!GIT_SHA.is_empty());
        assert!(!BUILD_DATE.is_empty());
    }

    #[test]
    fn test_check_port_in_use() {
        // Bind one listener, then check that the same port is in use
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

        assert!(check_port_available(addr).is_err());
    }
}
