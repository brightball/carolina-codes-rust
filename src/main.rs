use std::collections::{HashMap, HashSet};
use std::env;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{middleware, Json, Router};
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use serde::Deserialize;
use serde::Serialize;
use serde_json::{json, Value};
use tokio_postgres::{NoTls, Row};
use url::Url;

const LANGUAGE: &str = "Rust";
const API_VERSION: &str = "0.2.0";
const FRAMEWORK: &str = "axum";
const CREATED_YEAR: i32 = 2026;
const SCHEMA_VERSION: i32 = 1;
const LANGUAGE_VERSION: &str = env!("RUSTC_VERSION");

const SPEAKER_COLS: &str = "slug, first_name, last_name, name, tagline, bio, company, location, \
     photo_path, twitter_url, linkedin_url, website_url, github_url, featured";
const YEAR_SPONSOR_COLS: &str =
    "slug, name, website, logo_path, description, blurb, tier, featured, year, \
     twitter_url, linkedin_url, youtube_url, instagram_url, facebook_url";
const SPONSOR_COLS: &str =
    "slug, name, website, logo_path, description, twitter_url, linkedin_url, \
     youtube_url, instagram_url, facebook_url";
const TALK_COLS: &str =
    "slug, title, description, format, youtube_id, year, speaker_slug, languages, topics";
const SPONSORSHIP_COLS: &str = "sponsor_slug, year, tier, blurb, featured";

static SQL_COUNT: AtomicU64 = AtomicU64::new(0);
static CONNECT_COUNT: AtomicU64 = AtomicU64::new(0);

#[allow(dead_code)]
fn reset_counts() {
    SQL_COUNT.store(0, Ordering::SeqCst);
    CONNECT_COUNT.store(0, Ordering::SeqCst);
}

fn listen_host() -> &'static str {
    "::"
}

fn listen_addr(port: u16) -> SocketAddr {
    SocketAddr::from((listen_host().parse::<Ipv6Addr>().expect("listen host is IPv6"), port))
}

#[derive(Clone)]
struct AppState {
    pool: Pool,
}

#[derive(Deserialize, Default)]
struct YearQuery {
    year: Option<i64>,
}

#[derive(Serialize)]
struct Endpoint {
    method: &'static str,
    path: &'static str,
    query: &'static [&'static str],
}

const ENDPOINTS: &[Endpoint] = &[
    Endpoint {
        method: "GET",
        path: "/",
        query: &[],
    },
    Endpoint {
        method: "GET",
        path: "/health",
        query: &[],
    },
    Endpoint {
        method: "GET",
        path: "/v1/years",
        query: &[],
    },
    Endpoint {
        method: "GET",
        path: "/v1/speakers",
        query: &["year"],
    },
    Endpoint {
        method: "GET",
        path: "/v1/speakers/:slug",
        query: &[],
    },
    Endpoint {
        method: "GET",
        path: "/v1/speakers/:year/:slug",
        query: &[],
    },
    Endpoint {
        method: "GET",
        path: "/v1/sponsors",
        query: &["year"],
    },
    Endpoint {
        method: "GET",
        path: "/v1/sponsors/:slug",
        query: &[],
    },
    Endpoint {
        method: "GET",
        path: "/v1/sponsors/:year/:slug",
        query: &[],
    },
];

fn identity() -> Value {
    json!({
        "language": LANGUAGE,
        "language_version": LANGUAGE_VERSION,
        "api_version": API_VERSION,
        "framework": FRAMEWORK,
        "created_year": CREATED_YEAR,
        "schema_version": SCHEMA_VERSION,
        "endpoints": ENDPOINTS,
    })
}

#[tokio::main]
async fn main() {
    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:5432/carolina_dev".into());
    let pool = db_pool(&database_url).unwrap_or_else(|err| {
        eprintln!("database config: {err}");
        std::process::exit(1);
    });

    let app = router(AppState { pool });

    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4005);
    tokio::spawn(register(port));

    let addr = listen_addr(port);
    eprintln!("carolina-codes-rust listening on :{port}");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|err| {
            eprintln!("bind: {err}");
            std::process::exit(1);
        });
    axum::serve(listener, app).await.unwrap_or_else(|err| {
        eprintln!("server: {err}");
        std::process::exit(1);
    });
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/v1/years", get(years))
        .route("/v1/speakers", get(speakers))
        .route("/v1/speakers/{year}/{slug}", get(speaker_year))
        .route("/v1/speakers/{slug}", get(speaker_slug))
        .route("/v1/sponsors", get(sponsors))
        .route("/v1/sponsors/{year}/{slug}", get(sponsor_year))
        .route("/v1/sponsors/{slug}", get(sponsor_slug))
        .fallback(not_found)
        .layer(middleware::map_response(polyglot_headers))
        .with_state(state)
}

async fn db_query(
    client: &deadpool_postgres::Client,
    sql: &str,
    params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
) -> Result<Vec<Row>, ApiError> {
    SQL_COUNT.fetch_add(1, Ordering::SeqCst);
    Ok(client.query(sql, params).await?)
}

async fn db_query_opt(
    client: &deadpool_postgres::Client,
    sql: &str,
    params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
) -> Result<Option<Row>, ApiError> {
    SQL_COUNT.fetch_add(1, Ordering::SeqCst);
    Ok(client.query_opt(sql, params).await?)
}

async fn polyglot_headers(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert("X-Polyglot-Language", HeaderValue::from_static(LANGUAGE));
    response
        .headers_mut()
        .insert("X-Polyglot-Framework", HeaderValue::from_static(FRAMEWORK));
    response
}

async fn root() -> Json<Value> {
    Json(identity())
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true }))
}

async fn not_found() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" })))
}

async fn years(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let client = state.pool.get().await?;
    let rows = db_query(
        &client,
        "SELECT year, slug, name, status FROM v1_years ORDER BY year DESC",
        &[],
    )
    .await?;
    let data: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "year": year_value(row),
                "slug": row.get::<_, String>("slug"),
                "name": row.get::<_, String>("name"),
                "status": row.get::<_, String>("status"),
            })
        })
        .collect();
    Ok(Json(json!({ "data": data })))
}

async fn speakers(
    State(state): State<AppState>,
    Query(query): Query<YearQuery>,
) -> Result<Json<Value>, ApiError> {
    let client = state.pool.get().await?;
    let data = list_speakers(&client, query.year).await?;
    Ok(Json(json!({ "data": data })))
}

async fn speaker_year(
    State(state): State<AppState>,
    Path((year, slug)): Path<(i64, String)>,
) -> Result<Json<Value>, ApiError> {
    let client = state.pool.get().await?;
    let mut speaker = load_speaker(&client, &slug)
        .await?
        .ok_or(ApiError::NotFound)?;
    let talks = load_talks(&client, &slug, Some(year)).await?;
    if talks.is_empty() {
        return Err(ApiError::NotFound);
    }
    let years = talk_years(&client, &slug).await?;
    speaker["year"] = json!(year);
    speaker["years"] = json!(years);
    speaker["other_years"] = json!(except_year(&years, year));
    speaker["talks"] = json!(talks);
    speaker["languages"] = json!(uniq_tags(&talks, "languages"));
    speaker["topics"] = json!(uniq_tags(&talks, "topics"));
    Ok(Json(json!({ "data": speaker })))
}

async fn speaker_slug(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let client = state.pool.get().await?;
    let mut speaker = load_speaker(&client, &slug)
        .await?
        .ok_or(ApiError::NotFound)?;
    speaker["talks"] = json!(load_talks(&client, &slug, None).await?);
    speaker["years"] = json!(talk_years(&client, &slug).await?);
    Ok(Json(json!({ "data": speaker })))
}

async fn sponsors(
    State(state): State<AppState>,
    Query(query): Query<YearQuery>,
) -> Result<Json<Value>, ApiError> {
    let client = state.pool.get().await?;
    let data = if let Some(year) = query.year {
        let sql = format!(
            "SELECT {YEAR_SPONSOR_COLS} FROM v1_year_sponsors WHERE year = $1 ORDER BY name"
        );
        let rows = db_query(&client, &sql, &[&year]).await?;
        rows.iter().map(year_sponsor_from_row).collect::<Vec<_>>()
    } else {
        let sql = format!("SELECT {SPONSOR_COLS} FROM v1_sponsors ORDER BY name");
        let rows = db_query(&client, &sql, &[]).await?;
        rows.iter().map(sponsor_from_row).collect::<Vec<_>>()
    };
    Ok(Json(json!({ "data": data })))
}

async fn sponsor_year(
    State(state): State<AppState>,
    Path((year, slug)): Path<(i64, String)>,
) -> Result<Json<Value>, ApiError> {
    let client = state.pool.get().await?;
    let sql =
        format!("SELECT {YEAR_SPONSOR_COLS} FROM v1_year_sponsors WHERE year = $1 AND slug = $2");
    let row = db_query_opt(&client, &sql, &[&year, &slug])
        .await?
        .ok_or(ApiError::NotFound)?;
    let mut sponsor = year_sponsor_from_row(&row);
    let years = sponsor_years(&client, &slug).await?;
    sponsor["years"] = json!(years);
    sponsor["other_years"] = json!(except_year(&years, year));
    Ok(Json(json!({ "data": sponsor })))
}

async fn sponsor_slug(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let client = state.pool.get().await?;
    let sql = format!("SELECT {SPONSOR_COLS} FROM v1_sponsors WHERE slug = $1");
    let row = db_query_opt(&client, &sql, &[&slug])
        .await?
        .ok_or(ApiError::NotFound)?;
    let mut sponsor = sponsor_from_row(&row);
    let sql = format!("SELECT {SPONSORSHIP_COLS} FROM v1_sponsorships WHERE sponsor_slug = $1");
    let rows = db_query(&client, &sql, &[&slug]).await?;
    sponsor["sponsorships"] = json!(rows.iter().map(sponsorship_from_row).collect::<Vec<_>>());
    Ok(Json(json!({ "data": sponsor })))
}

async fn list_speakers(
    client: &deadpool_postgres::Client,
    year: Option<i64>,
) -> Result<Vec<Value>, ApiError> {
    if let Some(year) = year {
        let sql = format!(
            "SELECT {SPEAKER_COLS} FROM v1_speakers \
             WHERE slug IN (SELECT speaker_slug FROM v1_talks WHERE year = $1) \
             ORDER BY last_name, first_name"
        );
        let rows = db_query(client, &sql, &[&year]).await?;
        let mut speakers: Vec<Value> = rows.iter().map(speaker_from_row).collect();
        attach_year_tags(client, &mut speakers, year).await?;
        return Ok(speakers);
    }
    let sql = format!("SELECT {SPEAKER_COLS} FROM v1_speakers ORDER BY last_name, first_name");
    let rows = db_query(client, &sql, &[]).await?;
    Ok(rows.iter().map(speaker_from_row).collect())
}

async fn attach_year_tags(
    client: &deadpool_postgres::Client,
    speakers: &mut [Value],
    year: i64,
) -> Result<(), ApiError> {
    if speakers.is_empty() {
        return Ok(());
    }
    let slugs: Vec<String> = speakers
        .iter()
        .filter_map(|s| s.get("slug").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    let talks_by = load_talks_for_year(client, year).await?;
    let years_by = load_years_for_slugs(client, &slugs).await?;
    for speaker in speakers.iter_mut() {
        let slug = speaker
            .get("slug")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let talks = talks_by.get(&slug).cloned().unwrap_or_default();
        let years = years_by.get(&slug).cloned().unwrap_or_default();
        speaker["year"] = json!(year);
        speaker["talks"] = json!(talks);
        speaker["languages"] = json!(uniq_tags(&talks, "languages"));
        speaker["topics"] = json!(uniq_tags(&talks, "topics"));
        speaker["years"] = json!(years);
    }
    Ok(())
}

async fn load_talks_for_year(
    client: &deadpool_postgres::Client,
    year: i64,
) -> Result<HashMap<String, Vec<Value>>, ApiError> {
    let sql = format!(
        "SELECT {TALK_COLS} FROM v1_talks WHERE year = $1 ORDER BY speaker_slug, year DESC"
    );
    let rows = db_query(client, &sql, &[&year]).await?;
    let mut out: HashMap<String, Vec<Value>> = HashMap::new();
    for row in rows {
        let talk = talk_from_row(&row);
        let slug = talk
            .get("speaker_slug")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.entry(slug).or_default().push(talk);
    }
    Ok(out)
}

async fn load_years_for_slugs(
    client: &deadpool_postgres::Client,
    slugs: &[String],
) -> Result<HashMap<String, Vec<i64>>, ApiError> {
    let mut out: HashMap<String, Vec<i64>> = HashMap::new();
    if slugs.is_empty() {
        return Ok(out);
    }
    let slug_list: Vec<String> = slugs.to_vec();
    let rows = db_query(
        client,
        "SELECT DISTINCT speaker_slug, year FROM v1_talks WHERE speaker_slug = ANY($1) ORDER BY speaker_slug, year DESC",
        &[&slug_list],
    )
    .await?;
    for row in rows {
        let slug: String = row.get("speaker_slug");
        out.entry(slug).or_default().push(year_value(&row));
    }
    Ok(out)
}

async fn load_speaker(
    client: &deadpool_postgres::Client,
    slug: &str,
) -> Result<Option<Value>, ApiError> {
    let sql = format!("SELECT {SPEAKER_COLS} FROM v1_speakers WHERE slug = $1");
    Ok(db_query_opt(client, &sql, &[&slug])
        .await?
        .map(|row| speaker_from_row(&row)))
}

async fn load_talks(
    client: &deadpool_postgres::Client,
    slug: &str,
    year: Option<i64>,
) -> Result<Vec<Value>, ApiError> {
    let sql_all =
        format!("SELECT {TALK_COLS} FROM v1_talks WHERE speaker_slug = $1 ORDER BY year DESC");
    let sql_year = format!(
        "SELECT {TALK_COLS} FROM v1_talks WHERE speaker_slug = $1 AND year = $2 ORDER BY year DESC"
    );
    let rows = if let Some(year) = year {
        db_query(client, &sql_year, &[&slug, &year]).await?
    } else {
        db_query(client, &sql_all, &[&slug]).await?
    };
    Ok(rows.iter().map(talk_from_row).collect())
}

async fn talk_years(client: &deadpool_postgres::Client, slug: &str) -> Result<Vec<i64>, ApiError> {
    let rows = db_query(
        client,
        "SELECT DISTINCT year FROM v1_talks WHERE speaker_slug = $1 ORDER BY year DESC",
        &[&slug],
    )
    .await?;
    Ok(rows.iter().map(year_value).collect())
}

async fn sponsor_years(
    client: &deadpool_postgres::Client,
    slug: &str,
) -> Result<Vec<i64>, ApiError> {
    let rows = db_query(
        client,
        "SELECT DISTINCT year FROM v1_sponsorships WHERE sponsor_slug = $1 ORDER BY year DESC",
        &[&slug],
    )
    .await?;
    Ok(rows.iter().map(year_value).collect())
}

fn speaker_from_row(row: &Row) -> Value {
    json!({
        "slug": row.get::<_, String>("slug"),
        "first_name": row.get::<_, String>("first_name"),
        "last_name": row.get::<_, String>("last_name"),
        "name": row.get::<_, String>("name"),
        "tagline": opt_str(row, "tagline"),
        "bio": opt_str(row, "bio"),
        "company": opt_str(row, "company"),
        "location": opt_str(row, "location"),
        "photo_path": opt_str(row, "photo_path"),
        "twitter_url": opt_str(row, "twitter_url"),
        "linkedin_url": opt_str(row, "linkedin_url"),
        "website_url": opt_str(row, "website_url"),
        "github_url": opt_str(row, "github_url"),
        "featured": row.get::<_, bool>("featured"),
    })
}

fn talk_from_row(row: &Row) -> Value {
    json!({
        "slug": row.get::<_, String>("slug"),
        "title": row.get::<_, String>("title"),
        "description": opt_str(row, "description"),
        "format": opt_str(row, "format"),
        "youtube_id": opt_str(row, "youtube_id"),
        "year": year_value(row),
        "speaker_slug": row.get::<_, String>("speaker_slug"),
        "languages": text_array(row, "languages"),
        "topics": text_array(row, "topics"),
    })
}

fn year_sponsor_from_row(row: &Row) -> Value {
    json!({
        "slug": row.get::<_, String>("slug"),
        "name": row.get::<_, String>("name"),
        "website": opt_str(row, "website"),
        "logo_path": opt_str(row, "logo_path"),
        "description": opt_str(row, "description"),
        "blurb": opt_str(row, "blurb"),
        "tier": opt_str(row, "tier"),
        "featured": row.get::<_, bool>("featured"),
        "year": year_value(row),
        "twitter_url": opt_str(row, "twitter_url"),
        "linkedin_url": opt_str(row, "linkedin_url"),
        "youtube_url": opt_str(row, "youtube_url"),
        "instagram_url": opt_str(row, "instagram_url"),
        "facebook_url": opt_str(row, "facebook_url"),
    })
}

fn sponsor_from_row(row: &Row) -> Value {
    json!({
        "slug": row.get::<_, String>("slug"),
        "name": row.get::<_, String>("name"),
        "website": opt_str(row, "website"),
        "logo_path": opt_str(row, "logo_path"),
        "description": opt_str(row, "description"),
        "twitter_url": opt_str(row, "twitter_url"),
        "linkedin_url": opt_str(row, "linkedin_url"),
        "youtube_url": opt_str(row, "youtube_url"),
        "instagram_url": opt_str(row, "instagram_url"),
        "facebook_url": opt_str(row, "facebook_url"),
    })
}

fn sponsorship_from_row(row: &Row) -> Value {
    json!({
        "sponsor_slug": row.get::<_, String>("sponsor_slug"),
        "year": year_value(row),
        "tier": opt_str(row, "tier"),
        "blurb": opt_str(row, "blurb"),
        "featured": row.get::<_, bool>("featured"),
    })
}

fn opt_str(row: &Row, col: &str) -> Option<String> {
    row.get(col)
}

fn text_array(row: &Row, col: &str) -> Vec<String> {
    row.try_get::<_, Option<Vec<String>>>(col)
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn year_value(row: &Row) -> i64 {
    if let Ok(year) = row.try_get::<_, i64>("year") {
        return year;
    }
    if let Ok(year) = row.try_get::<_, i32>("year") {
        return year as i64;
    }
    0
}

fn uniq_tags(talks: &[Value], key: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for talk in talks {
        if let Some(vals) = talk.get(key).and_then(|v| v.as_array()) {
            for val in vals {
                if let Some(s) = val.as_str() {
                    if !s.is_empty() && seen.insert(s.to_string()) {
                        out.push(s.to_string());
                    }
                }
            }
        }
    }
    out
}

fn except_year(years: &[i64], year: i64) -> Vec<i64> {
    years.iter().copied().filter(|y| *y != year).collect()
}

fn db_pool(database_url: &str) -> Result<Pool, Box<dyn std::error::Error + Send + Sync>> {
    CONNECT_COUNT.fetch_add(1, Ordering::SeqCst);
    let cfg = postgres_config(database_url)?;
    let mgr = Manager::from_config(
        cfg,
        NoTls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );
    Ok(Pool::builder(mgr)
        .max_size(16)
        .runtime(Runtime::Tokio1)
        .build()?)
}

fn postgres_config(
    database_url: &str,
) -> Result<tokio_postgres::Config, Box<dyn std::error::Error + Send + Sync>> {
    if !database_url.contains("://") {
        return Ok(database_url.parse()?);
    }
    let parsed = Url::parse(database_url)?;
    let mut cfg = tokio_postgres::Config::new();
    if let Some(host) = parsed.host_str() {
        cfg.host(host);
    }
    cfg.port(parsed.port().unwrap_or(5432));
    let dbname = parsed.path().trim_start_matches('/');
    if !dbname.is_empty() {
        cfg.dbname(dbname);
    }
    if !parsed.username().is_empty() {
        cfg.user(&pct_decode(parsed.username()));
    }
    if let Some(password) = parsed.password() {
        cfg.password(pct_decode(password));
    }
    Ok(cfg)
}

fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

async fn register(port: u16) {
    let url = match env::var("CAROLINA_URL") {
        Ok(v) if !v.is_empty() => v,
        _ => return,
    };
    let token = match env::var("POLYGLOT_REGISTER_TOKEN") {
        Ok(v) if !v.is_empty() => v,
        _ => return,
    };
    let base = env::var("PUBLIC_BASE_URL").unwrap_or_else(|_| format!("http://127.0.0.1:{port}"));
    let body = json!({
        "language": LANGUAGE,
        "language_version": LANGUAGE_VERSION,
        "api_version": API_VERSION,
        "framework": FRAMEWORK,
        "created_year": CREATED_YEAR,
        "schema_version": SCHEMA_VERSION,
        "base_url": base,
        "endpoints": ENDPOINTS,
    });
    let endpoint = format!(
        "{}/internal/api-endpoints/register",
        url.trim_end_matches('/')
    );
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            eprintln!("register: {err}");
            return;
        }
    };
    match client
        .post(endpoint)
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => eprintln!("registered with elixir: {}", resp.status()),
        Err(err) => eprintln!("register: {err}"),
    }
}

#[derive(Debug)]
enum ApiError {
    NotFound,
    Db(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::NotFound => {
                (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response()
            }
            ApiError::Db(msg) => {
                eprintln!("db: {msg}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": msg })),
                )
                    .into_response()
            }
        }
    }
}

impl From<tokio_postgres::Error> for ApiError {
    fn from(err: tokio_postgres::Error) -> Self {
        ApiError::Db(err.to_string())
    }
}

impl From<deadpool_postgres::PoolError> for ApiError {
    fn from(err: deadpool_postgres::PoolError) -> Self {
        ApiError::Db(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::sync::{Mutex, MutexGuard};
    use tower::ServiceExt;

    fn test_guard() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn src() -> &'static str {
        include_str!("main.rs")
    }

    fn assert_years_desc(speakers: &[Value], label: &str) {
        let mut found_multi = false;
        for sp in speakers {
            let years = sp
                .get("years")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_i64())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if years.len() < 2 {
                continue;
            }
            found_multi = true;
            for pair in years.windows(2) {
                assert!(
                    pair[0] >= pair[1],
                    "{label} years not DESC for {:?}: {years:?}",
                    sp.get("slug")
                );
            }
        }
        assert!(found_multi, "{label} expected a speaker with >=2 years");
    }

    #[test]
    fn listen_addr_is_ipv6() {
        assert_eq!(listen_host(), "::");
        assert!(listen_addr(4005).is_ipv6());
        let ipv4_any = format!("([{z}, {z}, {z}, {z}]", z = 0);
        assert!(!src().contains(&ipv4_any), "source still binds IPv4-only");
        assert!(src().contains("listen_addr("), "main should bind listen_addr");
        assert!(src().contains("NoTls"), "tokio-postgres stays NoTls");
        let reg = src()
            .split("async fn register(")
            .nth(1)
            .unwrap_or("");
        assert!(!reg.contains("db_query("), "register-once does not run catalog SQL");
        assert!(!reg.contains("db_pool("), "register-once does not open the pool");
        assert!(!reg.contains("pool.get("), "register-once does not check out a connection");
    }

    #[tokio::test]
    async fn health_does_not_query_or_connect() {
        let _guard = test_guard();
        reset_counts();
        let pool = db_pool(&test_db_url()).expect("open pool for state");
        let boot = CONNECT_COUNT.load(Ordering::SeqCst);
        SQL_COUNT.store(0, Ordering::SeqCst);
        let response = router(AppState { pool })
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(SQL_COUNT.load(Ordering::SeqCst), 0, "/health ran SQL");
        assert_eq!(
            CONNECT_COUNT.load(Ordering::SeqCst),
            boot,
            "/health opened Postgres"
        );
    }

    #[tokio::test]
    async fn year_listing_sql_bounded_and_years_desc() {
        let _guard = test_guard();
        reset_counts();
        let pool = db_pool(&test_db_url()).expect("live carolina_dev pool");
        let boot = CONNECT_COUNT.load(Ordering::SeqCst);
        SQL_COUNT.store(0, Ordering::SeqCst);
        let app = router(AppState { pool: pool.clone() });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/speakers?year=2026")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let payload: Value = serde_json::from_slice(&body).expect("json");
        let speakers = payload["data"].as_array().cloned().unwrap_or_default();
        let sql = SQL_COUNT.load(Ordering::SeqCst);
        eprintln!(
            "year list status={} sql={} speakers={} connects={}",
            status.as_u16(),
            sql,
            speakers.len(),
            CONNECT_COUNT.load(Ordering::SeqCst)
        );
        assert_eq!(status, StatusCode::OK, "live year listing {}", payload);
        assert!(speakers.len() >= 3, "year listing returns N>=3 speakers");
        assert!(sql > 0, "listing runs SQL through shipped query wrapper");
        assert!(
            sql < 2 * speakers.len() as u64,
            "SQL count {sql} grew like 2N for N={}",
            speakers.len()
        );
        assert!(sql <= 4, "year listing SQL {sql} should be speakers+talks+years");
        assert_years_desc(&speakers, "handler");
        assert_eq!(
            CONNECT_COUNT.load(Ordering::SeqCst),
            boot,
            "listing opened a new session"
        );

        let client = pool.get().await.expect("checkout");
        let rows = list_speakers(&client, Some(2026)).await.expect("list_speakers");
        assert_years_desc(&rows, "list_speakers");

        SQL_COUNT.store(0, Ordering::SeqCst);
        let second = app
            .oneshot(
                Request::builder()
                    .uri("/v1/speakers?year=2026")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(
            CONNECT_COUNT.load(Ordering::SeqCst),
            boot,
            "second catalog request opened a new session"
        );
    }

    fn test_db_url() -> String {
        env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:postgres@127.0.0.1:5432/carolina_dev".into()
        })
    }
}
