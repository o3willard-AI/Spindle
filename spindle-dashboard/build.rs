//! Build script for spindle-dashboard.
//!
//! The dashboard embeds the React frontend via rust-embed
//! (`#[folder = "../frontend/dist/"]`), which fails to compile when that
//! directory does not exist — i.e. on any bare `cargo build/test/clippy`
//! from a fresh clone or in CI where `bun run build` was never executed.
//!
//! To keep raw cargo usable, this script creates a STUB `index.html` (plus
//! the dist directory) ONLY when the real build output is missing. The stub
//! is never allowed to clobber a real SPA: if `frontend/dist/index.html`
//! already exists — because `make release` ran bun first, or a developer
//! built locally — this script does nothing and the REAL assets are embedded.
//!
//! The stub page renders a visible "frontend not built" notice so nobody
//! mistakes it for the real UI.

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    // CARGO_MANIFEST_DIR = spindle-dashboard/; frontend lives one level up.
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let dist_dir = Path::new(&manifest_dir)
        .parent()
        .expect("manifest dir has no parent")
        .join("frontend")
        .join("dist");
    let index_html = dist_dir.join("index.html");

    if index_html.exists() {
        // Real frontend build present — nothing to do. Never overwrite it.
        println!("cargo:rerun-if-changed={}", index_html.display());
        return;
    }

    println!(
        "cargo:warning=spindle-dashboard: frontend/dist/index.html missing — \
creating a stub so cargo works without bun. Run `cd frontend && bun install && bun run build` \
to embed the real SPA."
    );

    fs::create_dir_all(&dist_dir).expect("failed to create frontend/dist");

    let stub = r#"<!doctype html>
<html lang="en">
  <head><meta charset="utf-8"><title>Spindle Dashboard — frontend not built</title></head>
  <body style="font-family: system-ui, sans-serif; background:#111827; color:#f9fafb;
               display:flex; align-items:center; justify-content:center; height:100vh; margin:0">
    <div style="text-align:center">
      <h1>Spindle Dashboard</h1>
      <p>The web frontend has not been built into <code>frontend/dist/</code>.</p>
      <p>This is a placeholder embedded at compile time.</p>
      <p><code>cd frontend &amp;&amp; bun install &amp;&amp; bun run build</code>, then rebuild this binary.</p>
    </div>
  </body>
</html>
"#;
    fs::write(&index_html, stub).expect("failed to write stub index.html");

    // Re-run if the stub is removed (e.g. `bun run build` wipes dist/ via
    // emptyOutDir and produces the real index.html).
    println!("cargo:rerun-if-changed={}", index_html.display());
}
