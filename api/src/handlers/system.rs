use crate::db::Db;
use axum::{extract::State, http::StatusCode, response::Html, Json};
use serde_json::{json, Value};
use std::sync::OnceLock;

static INDEX_JSON: OnceLock<Value> = OnceLock::new();

const LANDING_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Solana Verified Builds</title>
    <style>
      :root { color-scheme: light dark; }
      body { font-family: ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, Helvetica, Arial, "Apple Color Emoji", "Segoe UI Emoji"; line-height: 1.5; margin: 0; }
      main { max-width: 880px; margin: 0 auto; padding: 56px 20px; }
      h1 { font-size: 32px; margin: 0 0 12px; }
      p { margin: 0 0 16px; }
      .card { border: 1px solid rgba(127,127,127,.25); border-radius: 12px; padding: 16px; margin: 18px 0; }
      .muted { opacity: .85; }
      a { color: inherit; }
      ul { margin: 10px 0 0 18px; padding: 0; }
      footer { margin-top: 28px; font-size: 14px; opacity: .85; }
      code { font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace; }
    </style>
  </head>
  <body>
    <main>
      <h1>Solana Verifiable Build</h1>
      <p class="muted">
        Verified builds help users confirm that an on-chain Solana program matches its public source code.
      </p>

      <div class="card">
        <strong>Docs</strong>
        <p style="margin:0;">
          <a href="https://solana.com/docs/programs/verified-builds#how-do-i-create-verified-builds">
            Solana docs: Verified Builds (How do I create verified builds?)
          </a>
        </p>
        <p class="muted" style="margin-top:12px;">
          Build tool: <a href="https://github.com/solana-foundation/solana-verifiable-build">solana-verifiable-build</a>
        </p>
      </div>

      <footer>
        Looking for the API? See <code>GET /api</code> for the endpoint list.
      </footer>
    </main>
  </body>
</html>
"#;

pub async fn landing() -> Html<&'static str> {
    Html(LANDING_HTML)
}

pub async fn index() -> Json<Value> {
    let v = INDEX_JSON.get_or_init(|| {
        json!({
            "endpoints": [
                { "path": "/", "method": "GET", "description": "Landing page" },
                { "path": "/api", "method": "GET", "description": "API endpoint documentation" },
                { "path": "/verify", "method": "POST", "description": "Asynchronously verify a Solana program" },
                { "path": "/verify-with-signer", "method": "POST", "description": "Asynchronously verify using a specific signer" },
                { "path": "/verify_sync", "method": "POST", "description": "Synchronously verify a Solana program" },
                { "path": "/status/:program_id", "method": "GET", "description": "Verification status for a program" },
                { "path": "/status-all/:program_id", "method": "GET", "description": "All signers' verification claims for a program" },
                { "path": "/resolve-hash/:hash", "method": "GET", "description": "Content-addressed build lookup" },
                { "path": "/job/:job_id", "method": "GET", "description": "Build job status" },
                { "path": "/logs/:build_id", "method": "GET", "description": "Build logs" },
                { "path": "/verified-programs", "method": "GET", "description": "List of verified programs (page 1)" },
                { "path": "/verified-programs/:page", "method": "GET", "description": "Paginated list of verified programs" },
                { "path": "/verified-programs-status", "method": "GET", "description": "Status of all verified programs" },
                { "path": "/health", "method": "GET", "description": "Health check" },
                { "path": "/health/background-jobs", "method": "GET", "description": "Background sweep status" }
            ]
        })
    });
    Json(v.clone())
}

pub async fn health(State(db): State<Db>) -> (StatusCode, Json<Value>) {
    let db_ok = sqlx::query("SELECT 1").execute(&db.pool).await.is_ok();
    let last = db.last_sweep_at().await.ok().flatten();
    let now = chrono::Utc::now();
    let interval = chrono::Duration::seconds(crate::config::CONFIG.sweep_interval_seconds as i64);
    let sweep_ok = match last {
        Some(t) => now - t <= interval * 2,
        None => true,
    };
    let overall = db_ok && sweep_ok;
    let body = json!({
        "status": if overall { "ok" } else { "degraded" },
        "database": if db_ok { "connected" } else { "error" },
        "sweep": {
            "last_program_check": last.map(|t| t.naive_utc()),
            "ok": sweep_ok
        },
        "timestamp": now
    });
    (
        if overall {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(body),
    )
}
