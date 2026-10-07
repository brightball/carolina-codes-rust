use std::collections::{HashMap, HashSet};
use std::env;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, ToSocketAddrs};
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
use tokio_postgres::config::Host;
use tokio_postgres::{NoTls, Row};

const LANGUAGE: &str = "Rust";
const API_VERSION: &str = "0.2.0";
const FRAMEWORK: &str = "axum";
const CREATED_YEAR: i32 = 2026;
const SCHEMA_VERSION: i32 = 1;
const LANGUAGE_VERSION: &str = env!("RUSTC_VERSION");

const SQL_YEARS: &str = "SELECT year, slug, name, status FROM v1_years ORDER BY year DESC";
const SQL_SPEAKERS: &str = "SELECT slug, first_name, last_name, name, tagline, bio, company, \
     location, photo_path, twitter_url, linkedin_url, website_url, github_url, featured \
     FROM v1_speakers ORDER BY last_name, first_name";
const SQL_SPEAKERS_FOR_YEAR: &str = "SELECT slug, first_name, last_name, name, tagline, bio, \
     company, location, photo_path, twitter_url, linkedin_url, website_url, github_url, featured \
     FROM v1_speakers WHERE slug IN (SELECT speaker_slug FROM v1_talks WHERE year = $1) \
     ORDER BY last_name, first_name";
const SQL_SPEAKER_BY_SLUG: &str = "SELECT slug, first_name, last_name, name, tagline, bio, \
     company, location, photo_path, twitter_url, linkedin_url, website_url, github_url, featured \
     FROM v1_speakers WHERE slug = $1";
const SQL_TALKS_FOR_YEAR: &str = "SELECT slug, title, description, format, youtube_id, year, \
     speaker_slug, languages, topics FROM v1_talks WHERE year = $1 \
     ORDER BY speaker_slug, year DESC";
const SQL_TALKS_BY_SLUG: &str = "SELECT slug, title, description, format, youtube_id, year, \
     speaker_slug, languages, topics FROM v1_talks WHERE speaker_slug = $1 ORDER BY year DESC";
const SQL_TALKS_BY_SLUG_YEAR: &str = "SELECT slug, title, description, format, youtube_id, year, \
     speaker_slug, languages, topics FROM v1_talks WHERE speaker_slug = $1 AND year = $2 \
     ORDER BY year DESC";
const SQL_TALK_YEARS: &str =
    "SELECT DISTINCT year FROM v1_talks WHERE speaker_slug = $1 ORDER BY year DESC";
const SQL_YEARS_FOR_SLUGS: &str = "SELECT DISTINCT speaker_slug, year FROM v1_talks \
     WHERE speaker_slug = ANY($1) ORDER BY speaker_slug, year DESC";
const SQL_YEAR_SPONSORS: &str = "SELECT slug, name, website, logo_path, description, blurb, \
     tier, featured, year, twitter_url, linkedin_url, youtube_url, instagram_url, facebook_url \
     FROM v1_year_sponsors WHERE year = $1 ORDER BY name";
const SQL_YEAR_SPONSOR: &str = "SELECT slug, name, website, logo_path, description, blurb, \
     tier, featured, year, twitter_url, linkedin_url, youtube_url, instagram_url, facebook_url \
     FROM v1_year_sponsors WHERE year = $1 AND slug = $2";
const SQL_SPONSORS: &str = "SELECT slug, name, website, logo_path, description, twitter_url, \
     linkedin_url, youtube_url, instagram_url, facebook_url FROM v1_sponsors ORDER BY name";
const SQL_SPONSOR_BY_SLUG: &str = "SELECT slug, name, website, logo_path, description, \
     twitter_url, linkedin_url, youtube_url, instagram_url, facebook_url \
     FROM v1_sponsors WHERE slug = $1";
const SQL_SPONSORSHIPS: &str =
    "SELECT sponsor_slug, year, tier, blurb, featured FROM v1_sponsorships WHERE sponsor_slug = $1";
const SQL_SPONSOR_YEARS: &str =
    "SELECT DISTINCT year FROM v1_sponsorships WHERE sponsor_slug = $1 ORDER BY year DESC";

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
    SocketAddr::from((
        listen_host()
            .parse::<Ipv6Addr>()
            .expect("listen host is IPv6"),
        port,
    ))
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

struct Settings {
    database_url: String,
    port: u16,
    carolina_url: String,
    register_token: String,
    public_base_url: String,
}

impl Settings {
    fn from_env() -> Self {
        Self {
            database_url: env::var("DATABASE_URL").unwrap_or_else(|_| {
                "postgres://postgres:postgres@127.0.0.1:5432/carolina_dev".into()
            }),
            port: env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(4005),
            carolina_url: env::var("CAROLINA_URL").unwrap_or_default(),
            register_token: env::var("POLYGLOT_REGISTER_TOKEN").unwrap_or_default(),
            public_base_url: env::var("PUBLIC_BASE_URL").unwrap_or_default(),
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(err) = boot(Settings::from_env()).await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn boot(settings: Settings) -> Result<(), String> {
    let pool = db_pool(&settings.database_url).map_err(|err| format!("database config: {err}"))?;
    let app = router(AppState { pool: pool.clone() });
    let port = settings.port;
    // Bind before warmup so /health does not wait on the Postgres connect timeout.
    let listener = tokio::net::TcpListener::bind(listen_addr(port))
        .await
        .map_err(|err| format!("bind: {err}"))?;
    eprintln!("carolina-codes-rust listening on :{port}");
    tokio::spawn(async move {
        prewarm_pool(&pool).await;
    });
    tokio::spawn(register(settings));
    axum::serve(listener, app)
        .await
        .map_err(|err| format!("server: {err}"))?;
    Ok(())
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
    let stmt = client.prepare_cached(sql).await?;
    Ok(client.query(&stmt, params).await?)
}

async fn db_query_opt(
    client: &deadpool_postgres::Client,
    sql: &str,
    params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
) -> Result<Option<Row>, ApiError> {
    SQL_COUNT.fetch_add(1, Ordering::SeqCst);
    let stmt = client.prepare_cached(sql).await?;
    Ok(client.query_opt(&stmt, params).await?)
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
    let rows = db_query(&client, SQL_YEARS, &[]).await?;
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
        let rows = db_query(&client, SQL_YEAR_SPONSORS, &[&year]).await?;
        rows.iter().map(year_sponsor_from_row).collect::<Vec<_>>()
    } else {
        let rows = db_query(&client, SQL_SPONSORS, &[]).await?;
        rows.iter().map(sponsor_from_row).collect::<Vec<_>>()
    };
    Ok(Json(json!({ "data": data })))
}

async fn sponsor_year(
    State(state): State<AppState>,
    Path((year, slug)): Path<(i64, String)>,
) -> Result<Json<Value>, ApiError> {
    let client = state.pool.get().await?;
    let row = db_query_opt(&client, SQL_YEAR_SPONSOR, &[&year, &slug])
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
    let row = db_query_opt(&client, SQL_SPONSOR_BY_SLUG, &[&slug])
        .await?
        .ok_or(ApiError::NotFound)?;
    let mut sponsor = sponsor_from_row(&row);
    let rows = db_query(&client, SQL_SPONSORSHIPS, &[&slug]).await?;
    sponsor["sponsorships"] = json!(rows.iter().map(sponsorship_from_row).collect::<Vec<_>>());
    Ok(Json(json!({ "data": sponsor })))
}

async fn list_speakers(
    client: &deadpool_postgres::Client,
    year: Option<i64>,
) -> Result<Vec<Value>, ApiError> {
    if let Some(year) = year {
        let rows = db_query(client, SQL_SPEAKERS_FOR_YEAR, &[&year]).await?;
        let mut speakers: Vec<Value> = rows.iter().map(speaker_from_row).collect();
        attach_year_tags(client, &mut speakers, year).await?;
        return Ok(speakers);
    }
    let rows = db_query(client, SQL_SPEAKERS, &[]).await?;
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
    let rows = db_query(client, SQL_TALKS_FOR_YEAR, &[&year]).await?;
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
    let rows = db_query(client, SQL_YEARS_FOR_SLUGS, &[&slug_list]).await?;
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
    Ok(db_query_opt(client, SQL_SPEAKER_BY_SLUG, &[&slug])
        .await?
        .map(|row| speaker_from_row(&row)))
}

async fn load_talks(
    client: &deadpool_postgres::Client,
    slug: &str,
    year: Option<i64>,
) -> Result<Vec<Value>, ApiError> {
    let rows = if let Some(year) = year {
        db_query(client, SQL_TALKS_BY_SLUG_YEAR, &[&slug, &year]).await?
    } else {
        db_query(client, SQL_TALKS_BY_SLUG, &[&slug]).await?
    };
    Ok(rows.iter().map(talk_from_row).collect())
}

async fn talk_years(client: &deadpool_postgres::Client, slug: &str) -> Result<Vec<i64>, ApiError> {
    let rows = db_query(client, SQL_TALK_YEARS, &[&slug]).await?;
    Ok(rows.iter().map(year_value).collect())
}

async fn sponsor_years(
    client: &deadpool_postgres::Client,
    slug: &str,
) -> Result<Vec<i64>, ApiError> {
    let rows = db_query(client, SQL_SPONSOR_YEARS, &[&slug]).await?;
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
        .max_size(4)
        .runtime(Runtime::Tokio1)
        .build()?)
}

async fn prewarm_pool(pool: &Pool) {
    let mut held = Vec::with_capacity(2);
    for _ in 0..2 {
        match pool.get().await {
            Ok(client) => held.push(client),
            Err(err) => {
                eprintln!("prewarm: {err}");
                break;
            }
        }
    }
}

fn postgres_config(
    database_url: &str,
) -> Result<tokio_postgres::Config, Box<dyn std::error::Error + Send + Sync>> {
    let mut cfg: tokio_postgres::Config = database_url.parse()?;
    if cfg.get_connect_timeout().is_none() {
        cfg.connect_timeout(Duration::from_secs(3));
    }
    prefer_fly_ipv6(&mut cfg);
    Ok(cfg)
}

fn is_fly_pg_host(name: &str) -> bool {
    name.contains("flycast") || name.contains(".internal") || name.contains(".fly.io")
}

fn resolve_ipv6(host: &str, port: u16) -> Option<IpAddr> {
    (host, port)
        .to_socket_addrs()
        .ok()?
        .find(SocketAddr::is_ipv6)
        .map(|addr| addr.ip())
}

fn prefer_fly_ipv6(cfg: &mut tokio_postgres::Config) {
    if !cfg.get_hostaddrs().is_empty() {
        return;
    }
    let hosts: Vec<String> = cfg
        .get_hosts()
        .iter()
        .filter_map(|host| match host {
            Host::Tcp(name) if is_fly_pg_host(name) => Some(name.clone()),
            _ => None,
        })
        .collect();
    if hosts.is_empty() {
        return;
    }
    let ports = cfg.get_ports().to_vec();
    for (i, name) in hosts.iter().enumerate() {
        let port = ports
            .get(i)
            .copied()
            .or_else(|| ports.first().copied())
            .unwrap_or(5432);
        if let Some(ip) = resolve_ipv6(name, port) {
            cfg.hostaddr(ip);
        }
    }
}

async fn register(settings: Settings) {
    let url = settings.carolina_url;
    if url.is_empty() {
        return;
    }
    let token = settings.register_token;
    if token.is_empty() {
        return;
    }
    let base = if settings.public_base_url.is_empty() {
        format!("http://127.0.0.1:{}", settings.port)
    } else {
        settings.public_base_url
    };
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
    use std::future::Future;
    use std::thread;

    use axum::body::Body;
    use axum::http::{HeaderMap, Request, StatusCode};
    use http_body_util::BodyExt;
    use tokio::sync::{Mutex, MutexGuard};
    use tower::ServiceExt;

    async fn test_guard() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::const_new(());
        LOCK.lock().await
    }

    fn src() -> &'static str {
        include_str!("main.rs")
    }

    fn repo_file(rel: &str) -> String {
        std::fs::read_to_string(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel))
            .unwrap_or_else(|err| panic!("read {rel}: {err}"))
    }

    fn quoted_after<'a>(text: &'a str, key: &str) -> &'a str {
        let rest = text
            .split_once(key)
            .unwrap_or_else(|| panic!("missing {key}"))
            .1;
        let end = rest
            .find('"')
            .unwrap_or_else(|| panic!("missing end quote after {key}"));
        &rest[..end]
    }

    fn gitea_jobs(yaml: &str) -> HashMap<String, String> {
        let rest = yaml
            .split_once("\njobs:\n")
            .map(|(_, rest)| rest)
            .unwrap_or("");
        let mut jobs = HashMap::new();
        let mut current: Option<String> = None;
        let mut buf = String::new();
        for line in rest.lines() {
            if let Some(name) = line.strip_prefix("  ") {
                if !name.starts_with(' ') && !name.starts_with('#') && name.ends_with(':') {
                    if let Some(cur) = current.take() {
                        jobs.insert(cur, std::mem::take(&mut buf));
                    }
                    current = Some(name.trim_end_matches(':').to_string());
                    continue;
                }
            }
            if current.is_some() {
                buf.push_str(line);
                buf.push('\n');
            }
        }
        if let Some(cur) = current {
            jobs.insert(cur, buf);
        }
        jobs
    }

    fn job_needs(body: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut in_list = false;
        for line in body.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("needs:") {
                let rest = rest.trim();
                if rest.is_empty() {
                    in_list = true;
                    continue;
                }
                if rest.starts_with('[') {
                    for item in rest.trim_matches(|c| c == '[' || c == ']').split(',') {
                        let item = item.trim();
                        if !item.is_empty() {
                            out.push(item.to_string());
                        }
                    }
                } else {
                    out.push(rest.to_string());
                }
                in_list = false;
                continue;
            }
            if in_list {
                if let Some(item) = trimmed.strip_prefix("- ") {
                    out.push(item.trim().to_string());
                } else if !trimmed.is_empty() && !line.starts_with("      ") {
                    in_list = false;
                }
            }
        }
        out
    }

    fn assert_years_desc(speakers: &[Value], label: &str) {
        let mut found_multi = false;
        for sp in speakers {
            let years = sp
                .get("years")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_i64()).collect::<Vec<_>>())
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
        assert!(
            src().contains("listen_addr("),
            "main should bind listen_addr"
        );
        assert!(src().contains("NoTls"), "tokio-postgres stays NoTls");
        assert!(
            src().contains("prepare_cached"),
            "queries use the pool statement cache"
        );
        assert!(
            src().contains("current_thread"),
            "single-worker tokio runtime"
        );
        assert!(src().contains("max_size(4)"), "pool sized for 1 CPU");
        let reg = fn_body(src(), "async fn register(");
        assert!(
            !reg.contains("db_query("),
            "register-once does not run catalog SQL"
        );
        assert!(
            !reg.contains("db_pool("),
            "register-once does not open the pool"
        );
        assert!(
            !reg.contains("pool.get("),
            "register-once does not check out a connection"
        );
        let boot_body = fn_body(src(), "async fn boot(");
        let bind_at = boot_body
            .find("TcpListener::bind")
            .expect("boot binds the listener");
        let serve_at = boot_body.find("axum::serve").expect("boot serves");
        let prewarm_at = boot_body
            .find("prewarm_pool(")
            .expect("boot warms the pool");
        assert!(
            bind_at < prewarm_at && prewarm_at < serve_at,
            "bind, then spawn warmup, then serve"
        );
        let startup = &boot_body[bind_at..serve_at];
        assert!(
            startup.contains("spawn(async move"),
            "warmup must be spawned so accept is not blocked on Postgres"
        );
        assert!(
            startup.contains("spawn(register"),
            "registration must be spawned so accept is not blocked on the CMS"
        );
    }

    fn fn_body<'a>(src: &'a str, sig: &str) -> &'a str {
        let rest = src.split_once(sig).map(|(_, rest)| rest).unwrap_or("");
        let start = rest.find('{').unwrap_or(0);
        let mut depth = 0i32;
        for (i, b) in rest[start..].bytes().enumerate() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &rest[start..=start + i];
                    }
                }
                _ => {}
            }
        }
        &rest[start..]
    }

    #[test]
    fn postgres_config_parses_url_and_sets_timeout() {
        let cfg = postgres_config("postgres://u:p@127.0.0.1:5432/carolina_dev").expect("parse");
        assert_eq!(cfg.get_connect_timeout(), Some(&Duration::from_secs(3)));
        assert_eq!(cfg.get_user(), Some("u"));
        assert_eq!(cfg.get_dbname(), Some("carolina_dev"));
        assert!(cfg.get_hostaddrs().is_empty(), "loopback is not a Fly host");
        assert!(is_fly_pg_host("carolina-codes-db.flycast"));
        assert!(is_fly_pg_host("carolina-codes-db.internal"));
        assert!(!is_fly_pg_host("127.0.0.1"));
    }

    #[test]
    fn precommit_and_gitea_wire_five_checks() {
        let precommit = repo_file(".pre-commit-config.yaml");
        let workflow = repo_file(".gitea/workflows/precommit.yml");
        let makefile = repo_file("Makefile");
        let hook = repo_file(".githooks/pre-commit");
        let combined = format!("{precommit}\n{makefile}\n{workflow}\n{hook}");

        assert!(
            combined.contains("cargo test"),
            "tests must invoke cargo test"
        );
        assert!(
            combined.contains("clippy"),
            "static analysis must invoke clippy"
        );
        assert!(
            combined.contains("cargo audit"),
            "lockfile scan must invoke cargo audit"
        );
        assert!(
            combined.contains("gitleaks"),
            "secret scan must invoke gitleaks"
        );
        assert!(
            combined.contains("cargo fmt"),
            "style check must invoke cargo fmt"
        );
        assert!(
            precommit.contains("gitleaks"),
            "pre-commit must run gitleaks as the secret scanner"
        );
        assert!(
            workflow.contains("gitleaks"),
            "Gitea workflow must run gitleaks as the secret scanner"
        );
        for id in [
            "id: fmt",
            "id: clippy",
            "id: local-tests",
            "id: audit",
            "id: gitleaks",
        ] {
            assert!(precommit.contains(id), "pre-commit missing {id}");
        }

        let jobs = gitea_jobs(&workflow);
        let checks = ["test", "clippy", "audit", "gitleaks", "fmt"];
        let clone_cmd = r#"git clone --depth 1 --no-checkout "https://x-access-token:${token}@${host}/${GITHUB_REPOSITORY}" ."#;
        for name in checks {
            assert!(
                jobs.contains_key(name),
                "Gitea workflow missing distinct job {name}; jobs={:?}",
                jobs.keys().collect::<Vec<_>>()
            );
        }
        assert!(
            jobs.contains_key("prep"),
            "Gitea workflow missing prep job; jobs={:?}",
            jobs.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            jobs.len(),
            checks.len() + 1,
            "expected prep plus one Gitea job per check, got {:?}",
            jobs.keys().collect::<Vec<_>>()
        );
        assert!(
            !workflow.lines().any(|line| {
                let trimmed = line.trim();
                trimmed == "git init"
                    || trimmed.starts_with("git init ")
                    || trimmed.starts_with("- run: git init")
            }),
            "workflow must not git init"
        );
        assert!(
            !workflow.contains("git config --global init.defaultBranch"),
            "workflow must not set init.defaultBranch"
        );
        assert!(
            workflow.contains("cancel-in-progress: true"),
            "outdated precommit runs must be cancelled"
        );

        let prep = jobs.get("prep").expect("prep job body");
        assert!(
            job_needs(prep).is_empty(),
            "prep must not wait on a check job"
        );
        assert!(
            prep.contains(clone_cmd),
            "prep must clone the job workspace without git init"
        );
        assert!(
            prep.contains(r#"git fetch --depth 1 origin "${GITHUB_SHA}""#),
            "prep must fetch the SHA under test"
        );
        assert!(
            prep.contains("missing job token for git fetch"),
            "prep must fail closed if the job token is missing"
        );
        assert!(
            prep.contains("rustup component add clippy"),
            "prep must install clippy; rust: bookworm images ship rustup profile minimal"
        );
        assert!(
            prep.contains("rustup component add") && prep.contains("rustfmt"),
            "prep must install rustfmt; rust: bookworm images ship rustup profile minimal"
        );
        assert!(
            prep.contains("cargo install cargo-audit"),
            "prep must install cargo-audit"
        );
        assert!(prep.contains("gitleaks"), "prep must install gitleaks");
        assert!(
            prep.contains("tar -czf /tmp/prep-workspace.tar.gz"),
            "prep must pack the workspace"
        );
        assert!(
            prep.contains("--exclude=./.git"),
            "must not exclude deps/*/.git of git cargo deps"
        );
        assert!(
            !prep.contains("--exclude=.git\n") && !prep.contains("--exclude=.git "),
            "bare --exclude=.git would strip nested git dirs"
        );
        assert!(
            prep.contains("mv /tmp/prep-workspace.tar.gz prep-workspace.tar.gz"),
            "prep must move the tarball into the upload path"
        );
        assert!(
            prep.contains("actions/upload-artifact@v3"),
            "prep must upload the workspace artifact"
        );
        assert!(
            prep.contains("name: prep-workspace"),
            "prep must upload prep-workspace"
        );
        assert!(
            prep.contains(".ci-home/bin"),
            "prep must pack extra toolchain bins so they survive a fresh container"
        );
        for name in [
            "cargo-clippy",
            "clippy-driver",
            "cargo-fmt",
            "rustfmt",
            "cargo-audit",
            "gitleaks",
        ] {
            assert!(
                prep.contains(name),
                "prep must pack {name} into the workspace"
            );
        }

        let check_cmds = [
            ("test", "cargo test"),
            ("clippy", "cargo clippy"),
            ("audit", "cargo audit"),
            ("gitleaks", "gitleaks detect"),
            ("fmt", "cargo fmt"),
        ];
        for name in checks {
            let body = jobs.get(name).expect("job body");
            let deps = job_needs(body);
            assert!(
                deps.iter().any(|dep| dep == "prep"),
                "job {name} must wait for prep; needs={deps:?}"
            );
            for dep in &deps {
                assert!(
                    !checks.contains(&dep.as_str()),
                    "job {name} needs {dep} would serialize the five checks"
                );
            }
            assert!(
                body.contains("actions/download-artifact@v3"),
                "{name} must download the prep artifact"
            );
            assert!(
                body.contains("name: prep-workspace"),
                "{name} must restore prep-workspace"
            );
            assert!(
                body.contains("prep-workspace.tar.gz"),
                "{name} must unpack the prep workspace"
            );
            assert!(
                body.contains("scripts/ci-restore.sh"),
                "{name} must restore packed tools via scripts/ci-restore.sh"
            );
            assert!(
                !body.contains("cp -a .ci-home/bin/. /usr/local/cargo/bin/"),
                "{name} must not copy rustc-driver ELFs over rustup shims in cargo/bin"
            );
            assert!(
                !body.contains(clone_cmd),
                "{name} must not clone; restore the prep workspace"
            );
            assert!(
                !body.contains("missing job token for git fetch"),
                "{name} must not token-clone"
            );
            assert!(
                !body.contains("rustup component add"),
                "{name} must not rustup component add; restore tools from prep"
            );
            assert!(
                !body.contains("cargo install cargo-audit"),
                "{name} must not install cargo-audit; restore tools from prep"
            );
            assert!(
                !body.contains("gitleaks_8.30.1"),
                "{name} must not download gitleaks; restore tools from prep"
            );
        }
        assert!(
            jobs["test"].contains("postgres:16-alpine"),
            "test job must supply Postgres 16"
        );
        assert!(
            jobs["test"].contains("DATABASE_URL"),
            "test job must set DATABASE_URL for live listing tests"
        );
        for name in checks {
            let body = &jobs[name];
            for (other, cmd) in check_cmds {
                if other == name {
                    assert!(body.contains(cmd), "job {name} must run {cmd}");
                } else {
                    assert!(
                        !body.contains(cmd),
                        "job {name} also runs {cmd}; each check must be its own Gitea job"
                    );
                }
            }
        }
        for (check, cmd) in check_cmds {
            assert!(
                !prep.contains(cmd),
                "prep also runs {cmd}; keep prepare logic out of the checks"
            );
            let matching = jobs
                .iter()
                .filter(|(_, body)| body.contains(cmd))
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                matching,
                vec![check],
                "{cmd} must be the check in exactly one job, got {matching:?}"
            );
        }

        let restore = repo_file("scripts/ci-restore.sh");
        assert!(
            restore.contains("rustc --print sysroot"),
            "restore must place rustc-linked tools in the rustc sysroot"
        );
        assert!(
            restore.contains(r#"$sysroot/bin"#),
            "restore must copy clippy/rustfmt into sysroot/bin so RUNPATH $ORIGIN/../lib resolves"
        );
        for name in ["cargo-clippy", "clippy-driver", "cargo-fmt", "rustfmt"] {
            assert!(
                restore.contains(name),
                "restore must install {name} into the sysroot"
            );
        }
        assert!(
            !restore.contains("cp -a .ci-home/bin/. /usr/local/cargo/bin/"),
            "restore must not copy rustc-driver ELFs over rustup shims"
        );
        assert!(
            restore.contains("cargo-audit") && restore.contains("CARGO_HOME"),
            "restore may copy cargo-audit into cargo/bin; it has no librustc_driver RUNPATH"
        );
    }

    #[test]
    fn ci_restore_places_rustc_driver_bins_in_sysroot_not_cargo_bin() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let root = std::env::temp_dir().join(format!(
            "carolina-ci-restore-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let ws = root.join("ws");
        let packed = ws.join(".ci-home/bin");
        let sysroot = root.join("sysroot");
        let cargo_home = root.join("cargo");
        let cargo_bin = cargo_home.join("bin");
        let local_bin = root.join("local-bin");
        let stub_bin = root.join("stub-bin");
        let github_path = root.join("github-path");
        fs::create_dir_all(&packed).unwrap();
        fs::create_dir_all(&cargo_bin).unwrap();
        fs::create_dir_all(&local_bin).unwrap();
        fs::create_dir_all(&stub_bin).unwrap();
        fs::write(cargo_bin.join("cargo-clippy"), "rustup-shim\n").unwrap();
        for name in [
            "cargo-clippy",
            "clippy-driver",
            "cargo-fmt",
            "rustfmt",
            "cargo-audit",
            "gitleaks",
        ] {
            let path = packed.join(name);
            fs::write(&path, format!("packed-{name}\n")).unwrap();
            let mut packed_perm = fs::metadata(&path).unwrap().permissions();
            packed_perm.set_mode(0o755);
            fs::set_permissions(&path, packed_perm).unwrap();
        }
        let rustc_stub = stub_bin.join("rustc");
        fs::write(
            &rustc_stub,
            format!("#!/bin/sh\n[ \"$1\" = --print ] && [ \"$2\" = sysroot ] && echo '{}' && exit 0\nexit 1\n", sysroot.display()),
        )
        .unwrap();
        let mut perm = fs::metadata(&rustc_stub).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(&rustc_stub, perm).unwrap();

        let script = format!("{}/scripts/ci-restore.sh", env!("CARGO_MANIFEST_DIR"));
        let path = format!("{}:/usr/bin:/bin", stub_bin.display());
        let status = Command::new("sh")
            .arg(&script)
            .current_dir(&ws)
            .env("CI_RESTORE_LOCAL_BIN", &local_bin)
            .env("CARGO_HOME", &cargo_home)
            .env("GITHUB_PATH", &github_path)
            .env("PATH", &path)
            .status()
            .expect("run ci-restore.sh");
        assert!(status.success(), "ci-restore.sh failed: {status}");

        for name in ["cargo-clippy", "clippy-driver", "cargo-fmt", "rustfmt"] {
            let dest = sysroot.join("bin").join(name);
            let body = fs::read_to_string(&dest)
                .unwrap_or_else(|err| panic!("read {}: {err}", dest.display()));
            assert_eq!(
                body,
                format!("packed-{name}\n"),
                "{name} must land in sysroot/bin"
            );
            let cargo_copy = cargo_bin.join(name);
            if name == "cargo-clippy" {
                assert_eq!(
                    fs::read_to_string(&cargo_copy).unwrap(),
                    "rustup-shim\n",
                    "must not overwrite rustup shim in cargo/bin"
                );
            } else {
                assert!(
                    !cargo_copy.exists(),
                    "{name} must not be copied over rustup shims in cargo/bin"
                );
            }
            let wrapper = fs::read_to_string(local_bin.join(name)).unwrap();
            assert!(
                wrapper.contains(&format!("{}/bin/{name}", sysroot.display())),
                "{name} PATH wrapper must exec the sysroot ELF"
            );
            assert!(
                wrapper.contains("exec "),
                "{name} wrapper must exec so RUNPATH $ORIGIN is sysroot/bin"
            );
        }
        assert_eq!(
            fs::read_to_string(cargo_bin.join("cargo-audit")).unwrap(),
            "packed-cargo-audit\n"
        );
        assert_eq!(
            fs::read_to_string(local_bin.join("gitleaks")).unwrap(),
            "packed-gitleaks\n"
        );
        let github_path_body = fs::read_to_string(&github_path).unwrap();
        assert!(
            github_path_body.contains(&format!("{}/bin", sysroot.display())),
            "GITHUB_PATH must prepend sysroot/bin, got {github_path_body:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn health_does_not_query_or_connect() {
        let _guard = test_guard().await;
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
        assert_eq!(
            response
                .headers()
                .get("x-polyglot-language")
                .and_then(|v| v.to_str().ok()),
            Some("Rust")
        );
        assert_eq!(
            response
                .headers()
                .get("x-polyglot-framework")
                .and_then(|v| v.to_str().ok()),
            Some("axum")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v, json!({ "ok": true }));
        assert_eq!(SQL_COUNT.load(Ordering::SeqCst), 0, "/health ran SQL");
        assert_eq!(
            CONNECT_COUNT.load(Ordering::SeqCst),
            boot,
            "/health opened Postgres"
        );
    }

    const CATALOG_SQL: &str = r#"
CREATE TABLE years_src (
    year bigint PRIMARY KEY,
    slug text NOT NULL,
    name text NOT NULL,
    status text NOT NULL
);
CREATE TABLE speakers_src (
    slug text PRIMARY KEY,
    first_name text NOT NULL,
    last_name text NOT NULL,
    name text NOT NULL,
    tagline text,
    bio text,
    company text,
    location text,
    photo_path text,
    twitter_url text,
    linkedin_url text,
    website_url text,
    github_url text,
    featured boolean NOT NULL
);
CREATE TABLE talks_src (
    slug text PRIMARY KEY,
    title text NOT NULL,
    description text,
    format text,
    youtube_id text,
    year bigint NOT NULL,
    speaker_slug text NOT NULL,
    languages text[] NOT NULL,
    topics text[] NOT NULL
);
CREATE TABLE sponsors_src (
    slug text PRIMARY KEY,
    name text NOT NULL,
    website text,
    logo_path text,
    description text,
    twitter_url text,
    linkedin_url text,
    youtube_url text,
    instagram_url text,
    facebook_url text
);
CREATE TABLE sponsorships_src (
    sponsor_slug text NOT NULL,
    year bigint NOT NULL,
    tier text,
    blurb text,
    featured boolean NOT NULL
);
CREATE VIEW v1_years AS
    SELECT year, slug, name, status FROM years_src;
CREATE VIEW v1_speakers AS
    SELECT slug, first_name, last_name, name, tagline, bio, company, location,
           photo_path, twitter_url, linkedin_url, website_url, github_url, featured
    FROM speakers_src;
CREATE VIEW v1_talks AS
    SELECT slug, title, description, format, youtube_id, year, speaker_slug, languages, topics
    FROM talks_src;
CREATE VIEW v1_sponsors AS
    SELECT slug, name, website, logo_path, description,
           twitter_url, linkedin_url, youtube_url, instagram_url, facebook_url
    FROM sponsors_src;
CREATE VIEW v1_sponsorships AS
    SELECT sponsor_slug, year, tier, blurb, featured FROM sponsorships_src;
CREATE VIEW v1_year_sponsors AS
    SELECT s.slug, s.name, s.website, s.logo_path, s.description,
           p.blurb, p.tier, p.featured, p.year,
           s.twitter_url, s.linkedin_url, s.youtube_url, s.instagram_url, s.facebook_url
    FROM sponsors_src s
    JOIN sponsorships_src p ON p.sponsor_slug = s.slug;

INSERT INTO years_src (year, slug, name, status) VALUES
    (2026, '2026', 'Carolina Codes 2026', 'announced'),
    (2025, '2025', 'Carolina Codes 2025', 'completed'),
    (2024, '2024', 'Carolina Codes 2024', 'completed');

INSERT INTO speakers_src (
    slug, first_name, last_name, name, tagline, bio, company, location,
    photo_path, twitter_url, linkedin_url, website_url, github_url, featured
) VALUES
    ('ada', 'Ada', 'Lovelace', 'Ada Lovelace', 'Analyst', 'First programmer', 'Analytical Engines', 'London',
     '/photos/ada.jpg', NULL, NULL, 'https://ada.example', 'https://github.com/ada', true),
    ('grace', 'Grace', 'Hopper', 'Grace Hopper', NULL, NULL, 'Navy', 'Arlington',
     NULL, NULL, NULL, NULL, NULL, false),
    ('linus', 'Linus', 'Torvalds', 'Linus Torvalds', 'Kernel', 'Git and Linux', NULL, 'Portland',
     NULL, NULL, NULL, NULL, 'https://github.com/torvalds', false),
    ('edsger', 'Edsger', 'Dijkstra', 'Edsger Dijkstra', 'EWD', NULL, NULL, 'Eindhoven',
     NULL, NULL, NULL, NULL, NULL, false);

INSERT INTO talks_src (
    slug, title, description, format, youtube_id, year, speaker_slug, languages, topics
) VALUES
    ('ada-notes-2026', 'Notes on the engine', 'A talk', 'talk', 'yt-ada-2026', 2026, 'ada',
     ARRAY['rust']::text[], ARRAY['compilers','types']::text[]),
    ('ada-notes-2025', 'Notes on the engine again', NULL, 'talk', NULL, 2025, 'ada',
     ARRAY['rust','sql']::text[], ARRAY['databases']::text[]),
    ('grace-cobol-2026', 'Compilers aboard', 'COBOL', 'talk', NULL, 2026, 'grace',
     ARRAY['cobol']::text[], ARRAY['compilers']::text[]),
    ('linus-git-2026', 'Patches', NULL, 'talk', 'yt-linus', 2026, 'linus',
     ARRAY['c']::text[], ARRAY['vcs']::text[]),
    ('edsger-ewd-2024', 'Go to considered harmful', NULL, 'talk', NULL, 2024, 'edsger',
     ARRAY[]::text[], ARRAY['style']::text[]);

INSERT INTO sponsors_src (
    slug, name, website, logo_path, description,
    twitter_url, linkedin_url, youtube_url, instagram_url, facebook_url
) VALUES
    ('acme', 'Acme Corp', 'https://acme.example', '/logos/acme.png', 'Roadrunners',
     NULL, NULL, NULL, NULL, NULL),
    ('globex', 'Globex', 'https://globex.example', NULL, NULL,
     NULL, NULL, NULL, NULL, NULL);

INSERT INTO sponsorships_src (sponsor_slug, year, tier, blurb, featured) VALUES
    ('acme', 2026, 'gold', 'Premier', true),
    ('acme', 2025, 'silver', 'Returning', false),
    ('globex', 2026, 'bronze', NULL, false);
"#;

    struct IsolatedCatalog {
        dbname: String,
        admin_url: String,
        pool: Option<Pool>,
    }

    impl IsolatedCatalog {
        async fn open() -> Self {
            let dbname = fresh_dbname();
            let admin_url = with_dbname(&test_db_url(), "postgres");
            let admin = connect_pg(&admin_url).await;
            admin
                .batch_execute(&format!("CREATE DATABASE {dbname}"))
                .await
                .unwrap_or_else(|err| panic!("create isolated database {dbname}: {err}"));
            drop(admin);

            let catalog_url = with_dbname(&test_db_url(), &dbname);
            let client = connect_pg(&catalog_url).await;
            client
                .batch_execute(CATALOG_SQL)
                .await
                .unwrap_or_else(|err| panic!("seed {dbname}: {err}"));
            let rows = client
                .query(
                    "SELECT c.relname::text, c.relkind::text \
                     FROM pg_class c \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = 'public' AND c.relname LIKE 'v1\\_%' ESCAPE '\\' \
                     ORDER BY c.relname",
                    &[],
                )
                .await
                .unwrap_or_else(|err| panic!("list views in {dbname}: {err}"));
            let mut seen = Vec::new();
            for row in &rows {
                let name: String = row.get(0);
                let kind: String = row.get(1);
                assert_eq!(kind, "v", "{name} must be a view, relkind={kind}");
                seen.push(name);
            }
            for name in [
                "v1_speakers",
                "v1_sponsors",
                "v1_sponsorships",
                "v1_talks",
                "v1_year_sponsors",
                "v1_years",
            ] {
                assert!(
                    seen.iter().any(|got| got == name),
                    "missing view {name} in {seen:?}"
                );
            }
            drop(client);
            let pool = db_pool(&catalog_url).unwrap_or_else(|err| panic!("pool {dbname}: {err}"));
            Self {
                dbname,
                admin_url,
                pool: Some(pool),
            }
        }

        fn pool(&self) -> Pool {
            self.pool.clone().expect("isolated catalog pool")
        }
    }

    impl Drop for IsolatedCatalog {
        fn drop(&mut self) {
            self.pool.take();
            let dbname = std::mem::take(&mut self.dbname);
            if dbname.is_empty() {
                return;
            }
            let admin_url = self.admin_url.clone();
            let _ = thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("cleanup runtime");
                rt.block_on(async move {
                    let Ok(cfg) = postgres_config(&admin_url) else {
                        return;
                    };
                    let Ok((client, conn)) = cfg.connect(NoTls).await else {
                        return;
                    };
                    tokio::spawn(async move {
                        let _ = conn.await;
                    });
                    let _ = client
                        .batch_execute(&format!("DROP DATABASE IF EXISTS {dbname} WITH (FORCE)"))
                        .await;
                });
            })
            .join();
        }
    }

    async fn with_isolated_catalog<F, Fut>(body: F)
    where
        F: FnOnce(Pool) -> Fut,
        Fut: Future<Output = ()>,
    {
        let base = test_db_url();
        let before_rels = relation_names(&base).await;
        let admin_url = with_dbname(&base, "postgres");
        let before_dbs = database_names(&admin_url).await;
        let catalog = IsolatedCatalog::open().await;
        let pool = catalog.pool();
        body(pool).await;
        drop(catalog);
        let after_dbs = database_names(&admin_url).await;
        assert_eq!(
            before_dbs, after_dbs,
            "catalog fixture must not drop or leave databases on the shared server"
        );
        if let Some(before) = before_rels {
            let after = relation_names(&base)
                .await
                .expect("DATABASE_URL database disappeared");
            assert_eq!(
                before, after,
                "catalog fixture must not alter relations in the DATABASE_URL database"
            );
        }
    }

    async fn connect_pg(url: &str) -> tokio_postgres::Client {
        let cfg = postgres_config(url).unwrap_or_else(|err| panic!("postgres config: {err}"));
        let (client, conn) = cfg.connect(NoTls).await.unwrap_or_else(|err| {
            panic!("Postgres is required for catalog tests and must not be skipped: {err:?}")
        });
        tokio::spawn(async move {
            let _ = conn.await;
        });
        client
    }

    async fn database_names(admin_url: &str) -> Vec<String> {
        let client = connect_pg(admin_url).await;
        let rows = client
            .query(
                "SELECT datname::text FROM pg_database ORDER BY datname",
                &[],
            )
            .await
            .unwrap_or_else(|err| panic!("list databases: {err}"));
        rows.iter().map(|row| row.get(0)).collect()
    }

    async fn relation_names(url: &str) -> Option<Vec<String>> {
        let cfg = postgres_config(url).unwrap_or_else(|err| panic!("postgres config: {err}"));
        match cfg.connect(NoTls).await {
            Ok((client, conn)) => {
                tokio::spawn(async move {
                    let _ = conn.await;
                });
                let rows = client
                    .query(
                        "SELECT n.nspname || '.' || c.relname || ':' || c.relkind::text \
                         FROM pg_class c \
                         JOIN pg_namespace n ON n.oid = c.relnamespace \
                         WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') \
                           AND c.relkind IN ('r', 'v', 'm', 'p') \
                         ORDER BY 1",
                        &[],
                    )
                    .await
                    .unwrap_or_else(|err| panic!("list relations: {err}"));
                Some(rows.iter().map(|row| row.get(0)).collect())
            }
            Err(err) => {
                let missing_db = err.as_db_error().is_some_and(|db| {
                    db.code().code() == "3D000" || db.message().contains("does not exist")
                });
                if missing_db {
                    None
                } else {
                    panic!(
                        "Postgres is required for catalog tests and must not be skipped: {err:?}"
                    )
                }
            }
        }
    }

    fn fresh_dbname() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let name = format!("ccrust_{}_{nanos}", std::process::id());
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "unexpected database name {name}"
        );
        name
    }

    fn with_dbname(url: &str, dbname: &str) -> String {
        let (base, query) = match url.split_once('?') {
            Some((base, query)) => (base, Some(query)),
            None => (url, None),
        };
        let scheme = base.find("://").map(|i| i + 3).unwrap_or(0);
        let path = base[scheme..]
            .find('/')
            .map(|rel| scheme + rel)
            .unwrap_or(base.len());
        let mut out = format!("{}/{dbname}", base[..path].trim_end_matches('/'));
        if let Some(query) = query {
            out.push('?');
            out.push_str(query);
        }
        out
    }

    async fn oneshot_json(app: &Router, uri: &str) -> (StatusCode, HeaderMap, Value) {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap_or_else(|err| panic!("{uri}: {err}"));
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("{uri}: {err}; body={bytes:?}"));
        (status, headers, value)
    }

    fn assert_polyglot(headers: &HeaderMap, uri: &str) {
        let lang = headers
            .get("x-polyglot-language")
            .unwrap_or_else(|| panic!("{uri} missing X-Polyglot-Language: {headers:?}"))
            .to_str()
            .unwrap();
        let framework = headers
            .get("x-polyglot-framework")
            .unwrap_or_else(|| panic!("{uri} missing X-Polyglot-Framework: {headers:?}"))
            .to_str()
            .unwrap();
        assert_eq!(lang, "Rust", "{uri}");
        assert_eq!(framework, "axum", "{uri}");
    }

    fn data_array(body: &Value) -> &Vec<Value> {
        body.get("data")
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("expected data array in {body}"))
    }

    fn data_object(body: &Value) -> &Value {
        let data = body
            .get("data")
            .unwrap_or_else(|| panic!("expected data in {body}"));
        assert!(data.is_object(), "expected data object in {body}");
        data
    }

    fn years_of(value: &Value, key: &str) -> Vec<i64> {
        value
            .get(key)
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("missing {key} in {value}"))
            .iter()
            .filter_map(|v| v.as_i64())
            .collect()
    }

    fn by_slug<'a>(rows: &'a [Value], slug: &str) -> &'a Value {
        rows.iter()
            .find(|row| row["slug"] == slug)
            .unwrap_or_else(|| panic!("missing {slug}"))
    }

    #[test]
    fn with_dbname_rewrites_only_the_database() {
        assert_eq!(
            with_dbname(
                "postgres://postgres:postgres@127.0.0.1:5432/carolina_dev",
                "postgres"
            ),
            "postgres://postgres:postgres@127.0.0.1:5432/postgres"
        );
        assert_eq!(
            with_dbname(
                "postgres://postgres:postgres@postgres:5432/carolina_dev?sslmode=disable",
                "ccrust_1"
            ),
            "postgres://postgres:postgres@postgres:5432/ccrust_1?sslmode=disable"
        );
    }

    #[test]
    fn catalog_sql_targets_v1_views_only() {
        let queries = [
            SQL_YEARS,
            SQL_SPEAKERS,
            SQL_SPEAKERS_FOR_YEAR,
            SQL_SPEAKER_BY_SLUG,
            SQL_TALKS_FOR_YEAR,
            SQL_TALKS_BY_SLUG,
            SQL_TALKS_BY_SLUG_YEAR,
            SQL_TALK_YEARS,
            SQL_YEARS_FOR_SLUGS,
            SQL_YEAR_SPONSORS,
            SQL_YEAR_SPONSOR,
            SQL_SPONSORS,
            SQL_SPONSOR_BY_SLUG,
            SQL_SPONSORSHIPS,
            SQL_SPONSOR_YEARS,
        ];
        for sql in queries {
            let lower = sql.to_ascii_lowercase();
            assert!(lower.contains("v1_"), "{sql}");
            assert!(!lower.contains("ash"), "{sql}");
        }
    }

    #[test]
    fn docs_name_versions_memory_and_v1_views() {
        let readme = repo_file("README.md");
        let agents = repo_file("AGENTS.md");
        let decisions = repo_file("DECISIONS.md");
        let memory = repo_file("MEMORY.md");
        let toolchain = repo_file("rust-toolchain.toml");
        let cargo = repo_file("Cargo.toml");
        let docker = repo_file("Dockerfile");

        let rustc = quoted_after(&toolchain, "channel = \"");
        assert!(readme.contains("1.98.1"), "README must name Rust 1.98.1");
        assert!(
            readme.contains(rustc),
            "README must name the rust-toolchain.toml channel {rustc}"
        );
        assert!(
            docker.contains(&format!("rust:{rustc}-bookworm")),
            "Dockerfile must build with the pinned toolchain {rustc}"
        );

        let axum = quoted_after(&cargo, "axum = \"");
        assert!(readme.contains("0.8"), "README must name axum 0.8");
        assert!(
            readme.contains(axum) && readme.contains("axum"),
            "README must name the Cargo.toml axum version {axum}"
        );
        for name in [
            "tokio",
            "tokio-postgres",
            "deadpool-postgres",
            "reqwest",
            "rustls",
        ] {
            assert!(readme.contains(name), "README must name {name}");
        }
        assert!(
            !readme.to_ascii_lowercase().contains("crac"),
            "README must not claim CRaC"
        );

        assert!(
            agents.contains("MEMORY.md"),
            "AGENTS.md must name MEMORY.md"
        );
        assert!(
            agents.contains("DECISIONS.md"),
            "AGENTS.md must name DECISIONS.md"
        );
        assert!(
            decisions.contains("v1_") && decisions.contains("Ash"),
            "DECISIONS.md must record querying v1_* views rather than Ash tables"
        );
        assert!(
            memory.contains("axum") && decisions.contains("axum"),
            "MEMORY.md and DECISIONS.md must name the axum stack"
        );

        for (label, doc) in [
            ("README.md", &readme),
            ("AGENTS.md", &agents),
            ("DECISIONS.md", &decisions),
            ("MEMORY.md", &memory),
        ] {
            assert!(
                !doc.contains("zebra-hydra"),
                "{label} must not name the tailnet"
            );
            assert!(
                !doc.contains("ts.net"),
                "{label} must not name a tailnet host"
            );
            assert!(
                !doc.contains("/home/"),
                "{label} must not contain a home path"
            );
            assert!(
                !doc.contains(".internal"),
                "{label} must not contain a Fly internal URL"
            );
            assert!(
                !doc.contains("flycast"),
                "{label} must not contain a Flycast host"
            );
        }
    }

    #[test]
    fn fly_idle_is_not_stop_from_zero_and_image_is_locked() {
        let fly = repo_file("fly.toml");
        let stops = fly.contains("auto_stop_machines = \"stop\"");
        let min_zero = fly
            .lines()
            .any(|line| line.trim() == "min_machines_running = 0");
        assert!(
            !(stops && min_zero),
            "fly.toml must not stop machines with min_machines_running = 0"
        );
        assert!(
            fly.contains("auto_stop_machines = \"suspend\""),
            "256mb machine must suspend so idle resume is not a full boot"
        );
        assert!(
            fly.contains("memory = \"256mb\""),
            "suspend stays within Fly's memory limit"
        );
        assert!(fly.contains("auto_start_machines = true"));
        assert!(fly.contains("method = \"GET\""));
        assert!(fly.contains("path = \"/health\""));

        let docker = repo_file("Dockerfile");
        assert!(
            !docker.contains("cargo build --release &&")
                && !docker.contains("cargo build --release\n"),
            "release builds must pass --locked"
        );
        assert_eq!(
            docker.matches("cargo build --release --locked").count(),
            2,
            "dependency warmup and the final build both use --locked"
        );
        let runtime = docker
            .rsplit_once("\nFROM ")
            .map(|(_, rest)| rest)
            .unwrap_or("");
        assert!(
            runtime.starts_with("debian:bookworm-slim"),
            "runtime stage must not ship the Rust toolchain: {runtime}"
        );
        assert!(runtime.contains("USER nobody"));
        assert!(!runtime.contains("rustup"));
        assert!(!runtime.contains("cargo"));
        assert!(!runtime.contains("rustc"));
    }

    #[test]
    fn toolchain_matches_gitea_image_and_local_rustc() {
        let pinned = env!("RUSTC_VERSION");
        let toolchain = repo_file("rust-toolchain.toml");
        assert!(
            toolchain.contains(&format!("channel = \"{pinned}\"")),
            "rust-toolchain.toml must pin the compiler running these tests ({pinned})"
        );
        let image = format!("docker.io/library/rust:{pinned}-bookworm");
        let workflow = repo_file(".gitea/workflows/precommit.yml");
        assert_eq!(
            workflow.matches(image.as_str()).count(),
            5,
            "prep, test, clippy, audit, and fmt must use {image}"
        );
        let docker = repo_file("Dockerfile");
        assert!(
            docker.contains(&format!("FROM rust:{pinned}-bookworm")),
            "release build image must match local rustc {pinned}"
        );
    }

    #[tokio::test]
    async fn year_listing_sql_bounded_and_years_desc() {
        let _guard = test_guard().await;
        with_isolated_catalog(|pool| async move {
            reset_counts();
            let boot = CONNECT_COUNT.load(Ordering::SeqCst);
            let app = router(AppState { pool: pool.clone() });
            let (status, headers, payload) = oneshot_json(&app, "/v1/speakers?year=2026").await;
            let speakers = data_array(&payload).clone();
            let sql = SQL_COUNT.load(Ordering::SeqCst);
            assert_eq!(status, StatusCode::OK, "live year listing {payload}");
            assert_polyglot(&headers, "/v1/speakers?year=2026");
            assert!(speakers.len() >= 3, "year listing returns N>=3 speakers");
            assert!(sql > 0, "listing runs SQL through shipped query wrapper");
            assert!(
                sql < 2 * speakers.len() as u64,
                "SQL count {sql} grew like 2N for N={}",
                speakers.len()
            );
            assert!(
                sql <= 4,
                "year listing SQL {sql} should be speakers+talks+years"
            );
            assert_years_desc(&speakers, "handler");
            assert_eq!(
                CONNECT_COUNT.load(Ordering::SeqCst),
                boot,
                "listing opened a new session"
            );

            let client = pool.get().await.expect("checkout");
            let rows = list_speakers(&client, Some(2026))
                .await
                .expect("list_speakers");
            assert_years_desc(&rows, "list_speakers");

            SQL_COUNT.store(0, Ordering::SeqCst);
            let (status, _, _) = oneshot_json(&app, "/v1/speakers?year=2026").await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                CONNECT_COUNT.load(Ordering::SeqCst),
                boot,
                "second catalog request opened a new session"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn published_routes_against_isolated_v1_views() {
        let _guard = test_guard().await;
        with_isolated_catalog(|pool| async move {
            reset_counts();
            let app = router(AppState { pool });

            let (status, headers, body) = oneshot_json(&app, "/").await;
            assert_eq!(status, StatusCode::OK);
            assert_polyglot(&headers, "/");
            assert_eq!(body["language"], "Rust");
            assert_eq!(body["framework"], "axum");

            let connects = CONNECT_COUNT.load(Ordering::SeqCst);
            SQL_COUNT.store(0, Ordering::SeqCst);
            let (status, headers, body) = oneshot_json(&app, "/health").await;
            assert_eq!(status, StatusCode::OK);
            assert_polyglot(&headers, "/health");
            assert_eq!(body, json!({ "ok": true }));
            assert_eq!(SQL_COUNT.load(Ordering::SeqCst), 0, "/health ran SQL");
            assert_eq!(CONNECT_COUNT.load(Ordering::SeqCst), connects);

            let (status, headers, body) = oneshot_json(&app, "/v1/years").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_polyglot(&headers, "/v1/years");
            assert_eq!(
                body["data"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| row["year"].as_i64().unwrap())
                    .collect::<Vec<_>>(),
                vec![2026, 2025, 2024]
            );

            let (status, headers, body) = oneshot_json(&app, "/v1/speakers").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_polyglot(&headers, "/v1/speakers");
            let speakers = data_array(&body);
            assert_eq!(speakers.len(), 4);
            assert_eq!(by_slug(speakers, "ada")["name"], "Ada Lovelace");
            assert_eq!(by_slug(speakers, "ada")["tagline"], "Analyst");
            assert!(by_slug(speakers, "grace")["tagline"].is_null());

            reset_counts();
            let connects = CONNECT_COUNT.load(Ordering::SeqCst);
            let (status, headers, body) = oneshot_json(&app, "/v1/speakers?year=2026").await;
            let sql = SQL_COUNT.load(Ordering::SeqCst);
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_polyglot(&headers, "/v1/speakers?year=2026");
            let year_speakers = data_array(&body);
            assert!(year_speakers.len() >= 3);
            assert!(sql > 0 && sql <= 4, "year listing SQL {sql}");
            assert_eq!(
                CONNECT_COUNT.load(Ordering::SeqCst),
                connects,
                "year listing opened a new session"
            );
            assert_years_desc(year_speakers, "published year listing");
            let ada = by_slug(year_speakers, "ada");
            assert_eq!(ada["year"], 2026);
            assert!(!ada["talks"].as_array().unwrap().is_empty());
            assert!(ada["years"].as_array().unwrap().len() >= 2);

            let (status, headers, body) = oneshot_json(&app, "/v1/speakers/ada").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_polyglot(&headers, "/v1/speakers/ada");
            let ada = data_object(&body);
            assert_eq!(ada["slug"], "ada");
            assert!(ada["talks"].as_array().unwrap().len() >= 2);
            assert_eq!(years_of(ada, "years"), vec![2026, 2025]);

            let (status, headers, body) = oneshot_json(&app, "/v1/speakers/2026/ada").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_polyglot(&headers, "/v1/speakers/2026/ada");
            let ada = data_object(&body);
            assert_eq!(ada["year"], 2026);
            assert!(!ada["talks"].as_array().unwrap().is_empty());
            assert_eq!(years_of(ada, "other_years"), vec![2025]);
            assert!(ada["languages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str() == Some("rust")));

            let (status, headers, body) = oneshot_json(&app, "/v1/sponsors").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_polyglot(&headers, "/v1/sponsors");
            assert_eq!(data_array(&body).len(), 2);

            let (status, headers, body) = oneshot_json(&app, "/v1/sponsors?year=2026").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_polyglot(&headers, "/v1/sponsors?year=2026");
            let sponsors = data_array(&body);
            assert_eq!(sponsors.len(), 2);
            assert!(sponsors.iter().all(|row| row["year"] == 2026));

            let (status, headers, body) = oneshot_json(&app, "/v1/sponsors/acme").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_polyglot(&headers, "/v1/sponsors/acme");
            let acme = data_object(&body);
            assert_eq!(acme["slug"], "acme");
            assert_eq!(acme["sponsorships"].as_array().unwrap().len(), 2);

            let (status, headers, body) = oneshot_json(&app, "/v1/sponsors/2026/acme").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_polyglot(&headers, "/v1/sponsors/2026/acme");
            let acme = data_object(&body);
            assert_eq!(acme["year"], 2026);
            assert_eq!(acme["tier"], "gold");
            assert_eq!(years_of(acme, "other_years"), vec![2025]);

            for path in [
                "/v1/speakers/missing-speaker",
                "/v1/sponsors/missing-sponsor",
                "/v1/speakers/2026/missing-speaker",
                "/v1/sponsors/2026/missing-sponsor",
                "/v1/speakers/2019/ada",
                "/not-a-route",
            ] {
                let (status, headers, body) = oneshot_json(&app, path).await;
                assert_eq!(status, StatusCode::NOT_FOUND, "{path} -> {body}");
                assert_eq!(body["error"], "not_found", "{path}");
                assert_polyglot(&headers, path);
            }
        })
        .await;
    }

    fn test_db_url() -> String {
        env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:5432/carolina_dev".into())
    }
}
