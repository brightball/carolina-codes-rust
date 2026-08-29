use std::collections::HashSet;
use std::env;
use std::net::SocketAddr;
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

    let app = Router::new()
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
        .with_state(AppState { pool });

    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4005);
    tokio::spawn(register(port));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
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
    let rows = client
        .query(
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
    if let Some(year) = query.year {
        let sql = format!(
            "SELECT {SPEAKER_COLS} FROM v1_speakers \
             WHERE slug IN (SELECT speaker_slug FROM v1_talks WHERE year = $1) \
             ORDER BY last_name, first_name"
        );
        let rows = client.query(&sql, &[&year]).await?;
        let mut speakers = Vec::with_capacity(rows.len());
        for row in rows {
            let mut speaker = speaker_from_row(&row);
            let talks =
                load_talks(&client, speaker["slug"].as_str().unwrap_or(""), Some(year)).await?;
            let years = talk_years(&client, speaker["slug"].as_str().unwrap_or("")).await?;
            speaker["year"] = json!(year);
            speaker["talks"] = json!(talks);
            speaker["languages"] = json!(uniq_tags(&talks, "languages"));
            speaker["topics"] = json!(uniq_tags(&talks, "topics"));
            speaker["years"] = json!(years);
            speakers.push(speaker);
        }
        return Ok(Json(json!({ "data": speakers })));
    }

    let sql = format!("SELECT {SPEAKER_COLS} FROM v1_speakers ORDER BY last_name, first_name");
    let rows = client.query(&sql, &[]).await?;
    let data: Vec<Value> = rows.iter().map(speaker_from_row).collect();
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
        let rows = client.query(&sql, &[&year]).await?;
        rows.iter().map(year_sponsor_from_row).collect::<Vec<_>>()
    } else {
        let sql = format!("SELECT {SPONSOR_COLS} FROM v1_sponsors ORDER BY name");
        let rows = client.query(&sql, &[]).await?;
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
    let row = client
        .query_opt(&sql, &[&year, &slug])
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
    let row = client
        .query_opt(&sql, &[&slug])
        .await?
        .ok_or(ApiError::NotFound)?;
    let mut sponsor = sponsor_from_row(&row);
    let sql = format!("SELECT {SPONSORSHIP_COLS} FROM v1_sponsorships WHERE sponsor_slug = $1");
    let rows = client.query(&sql, &[&slug]).await?;
    sponsor["sponsorships"] = json!(rows.iter().map(sponsorship_from_row).collect::<Vec<_>>());
    Ok(Json(json!({ "data": sponsor })))
}

async fn load_speaker(
    client: &deadpool_postgres::Client,
    slug: &str,
) -> Result<Option<Value>, ApiError> {
    let sql = format!("SELECT {SPEAKER_COLS} FROM v1_speakers WHERE slug = $1");
    Ok(client
        .query_opt(&sql, &[&slug])
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
        client.query(&sql_year, &[&slug, &year]).await?
    } else {
        client.query(&sql_all, &[&slug]).await?
    };
    Ok(rows.iter().map(talk_from_row).collect())
}

async fn talk_years(client: &deadpool_postgres::Client, slug: &str) -> Result<Vec<i64>, ApiError> {
    let rows = client
        .query(
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
    let rows = client
        .query(
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
