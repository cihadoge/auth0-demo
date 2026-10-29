mod config;

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tower_http::compression::CompressionLayer;
use uuid::Uuid;

mod assets {
    use axum::body::Bytes;
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::UNIX_EPOCH;

    pub const INDEX: &str = include_str!("../static/index.html");
    pub const APP_JS: &str = include_str!("../static/app.js");
    pub const STYLE: &str = include_str!("../static/style.css");
    pub const MANIFEST: &str = include_str!("../static/manifest.webmanifest");
    pub const SW: &str = include_str!("../static/sw.js");
    pub const ICON: &str = include_str!("../static/icon.svg");

    const FILES: &[(&str, &[u8])] = &[
        ("index.html", INDEX.as_bytes()),
        ("app.js", APP_JS.as_bytes()),
        ("style.css", STYLE.as_bytes()),
        ("manifest.webmanifest", MANIFEST.as_bytes()),
        ("sw.js", SW.as_bytes()),
        ("icon.svg", ICON.as_bytes()),
    ];

    struct DiskCache {
        dir: PathBuf,
        stamp: u64,
        bodies: HashMap<&'static str, Bytes>,
    }

    static DISK_CACHE: Mutex<Option<DiskCache>> = Mutex::new(None);

    fn fnv_bytes(h: &mut u64, bytes: &[u8]) {
        for &b in bytes {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x100000001b3);
        }
    }

    fn fnv_u64(h: &mut u64, val: u64) {
        fnv_bytes(h, &val.to_le_bytes());
    }

    fn find_static_dir() -> Option<PathBuf> {
        let cwd = PathBuf::from("static");
        if cwd.join("index.html").is_file() {
            return Some(cwd);
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let dir = parent.join("static");
                if dir.join("index.html").is_file() {
                    return Some(dir);
                }
            }
        }
        None
    }

    fn dir_stamp(dir: &Path) -> Option<u64> {
        let mut h = 0xcbf29ce484222325u64;
        if let Ok(bytes) = fs::read(dir.join(".static-hash")) {
            fnv_bytes(&mut h, &bytes);
        }
        for &(rel, _) in FILES {
            let meta = fs::metadata(dir.join(rel)).ok()?;
            fnv_u64(&mut h, meta.len());
            if let Ok(mtime) = meta.modified() {
                if let Ok(dur) = mtime.duration_since(UNIX_EPOCH) {
                    fnv_u64(&mut h, dur.as_secs());
                    fnv_u64(&mut h, u64::from(dur.subsec_nanos()));
                }
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                fnv_u64(&mut h, meta.ino());
                fnv_u64(&mut h, meta.ctime() as u64);
                fnv_u64(&mut h, meta.ctime_nsec() as u64);
            }
        }
        Some(h)
    }

    pub fn get(rel: &'static str, fallback: &'static [u8]) -> Bytes {
        if let Some(dir) = find_static_dir() {
            if let Some(stamp) = dir_stamp(&dir) {
                let mut guard = DISK_CACHE.lock().unwrap();
                if let Some(cached) = guard.as_ref() {
                    if cached.dir == dir && cached.stamp == stamp {
                        if let Some(b) = cached.bodies.get(rel) {
                            return b.clone();
                        }
                    }
                }
                let mut bodies = HashMap::with_capacity(FILES.len());
                let mut ok = true;
                for &(name, _) in FILES {
                    match fs::read(dir.join(name)) {
                        Ok(bytes) => {
                            bodies.insert(name, Bytes::from(bytes));
                        }
                        Err(_) => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    let out = bodies.get(rel).cloned();
                    *guard = Some(DiskCache { dir, stamp, bodies });
                    if let Some(b) = out {
                        return b;
                    }
                }
            }
        }
        Bytes::from_static(fallback)
    }
}

#[derive(Clone)]
struct Session {
    user: serde_json::Value,
    access_token: String,
    id_token: Option<String>,
}

struct AppState {
    port: u16,
    name: &'static str,
    cfg: Mutex<config::Auth0Cfg>,
    config_path: Option<std::path::PathBuf>,
    client: reqwest::Client,
    sessions: Mutex<HashMap<String, Session>>,
    pending: Mutex<HashMap<String, u64>>,
}

const BROWSER_BLOCKED_PORTS: &[u16] = &[
    1, 7, 9, 11, 13, 15, 17, 19, 20, 21, 22, 23, 25, 37, 42, 43, 53, 69, 77, 79, 87, 95, 99,
    103, 104, 109, 110, 111, 113, 115, 117, 119, 123, 135, 137, 139, 143, 161, 179, 389, 427,
    465, 512, 513, 514, 515, 526, 530, 531, 532, 540, 548, 554, 556, 563, 587, 601, 636, 989,
    990, 993, 995, 1719, 1720, 1723, 2049, 3659, 4045, 4190, 5060, 5061, 6000, 6566, 6665,
    6666, 6667, 6668, 6669, 6679, 6697, 10080,
];

fn is_browser_blocked_port(p: u16) -> bool {
    BROWSER_BLOCKED_PORTS.contains(&p)
}

async fn bind_listener(host: &str, configured: u16) -> (tokio::net::TcpListener, u16) {
    if let Ok(p) = std::env::var("PORT") {
        if let Ok(port) = p.trim().parse::<u16>() {
            if port > 0 {
                match tokio::net::TcpListener::bind((host, port)).await {
                    Ok(l) => return (l, port),
                    Err(e) => eprintln!("PORT={port} bind failed: {e}"),
                }
            }
        }
    }
    if configured > 0 {
        match tokio::net::TcpListener::bind((host, configured)).await {
            Ok(l) => return (l, configured),
            Err(e) => eprintln!("config port {configured} bind failed: {e}"),
        }
    }
    for port in 69u16..=65535 {
        if is_browser_blocked_port(port) {
            continue;
        }
        match tokio::net::TcpListener::bind((host, port)).await {
            Ok(l) => return (l, port),
            Err(_) => continue,
        }
    }
    panic!("no available TCP port found after port 68");
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn first_forwarded(v: &str) -> &str {
    v.split(',').next().unwrap_or(v).trim()
}

fn base_url(headers: &HeaderMap) -> String {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .map(first_forwarded)
        .unwrap_or("localhost")
        .to_string();
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(first_forwarded)
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get("cf-visitor")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| {
                    // CF-Visitor: {"scheme":"https"}
                    s.find("\"scheme\"").and_then(|_| {
                        if s.contains("https") { Some("https".to_string()) } else { Some("http".to_string()) }
                    })
                })
        })
        .unwrap_or_else(|| {
            if host.starts_with("localhost") || host.starts_with("127.") {
                "http".to_string()
            } else {
                "https".to_string()
            }
        });
    format!("{proto}://{host}")
}

fn get_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(header::COOKIE)?.to_str().ok()?.split(';').find_map(|p| {
        let p = p.trim();
        let (k, v) = p.split_once('=')?;
        if k == name { Some(v.to_string()) } else { None }
    })
}

fn is_secure(headers: &HeaderMap) -> bool {
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(first_forwarded)
        .or_else(|| {
            headers
                .get("cf-visitor")
                .and_then(|v| v.to_str().ok())
                .map(|s| if s.contains("https") { "https" } else { "http" })
        })
        .unwrap_or_else(|| {
            if headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(|h| h.starts_with("localhost") || h.starts_with("127."))
                .unwrap_or(true)
            {
                "http"
            } else {
                "https"
            }
        });
    proto == "https"
}

async fn index() -> Html<axum::body::Bytes> {
    Html(assets::get("index.html", assets::INDEX.as_bytes()))
}

async fn app_js() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        assets::get("app.js", assets::APP_JS.as_bytes()),
    )
        .into_response()
}

async fn style_css() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        assets::get("style.css", assets::STYLE.as_bytes()),
    )
        .into_response()
}

async fn manifest() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/manifest+json"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        assets::get("manifest.webmanifest", assets::MANIFEST.as_bytes()),
    )
        .into_response()
}

async fn sw() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        assets::get("sw.js", assets::SW.as_bytes()),
    )
        .into_response()
}

async fn health(State(st): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let cfg = st.cfg.lock().unwrap().clone();
    Json(json!({
        "status": "ok",
        "service": st.name,
        "port": st.port,
        "auth0_configured": !cfg.domain.is_empty() && !cfg.client_id.is_empty(),
    }))
}


async fn get_config(State(st): State<Arc<AppState>>) -> Response {
    let path = st.config_path.clone().unwrap_or_else(|| std::path::PathBuf::from("config.toml"));
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    Json(json!({
        "path": path.display().to_string(),
        "content": content,
    })).into_response()
}

async fn post_config(State(st): State<Arc<AppState>>, body: axum::body::Bytes) -> Response {
    // Accept either JSON { "content": "..." } or a raw TOML body.
    let content = if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) {
        v.get("content")
            .or_else(|| v.get("config"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| String::from_utf8_lossy(&body).to_string())
    } else {
        String::from_utf8_lossy(&body).to_string()
    };

    let path = st.config_path.clone().unwrap_or_else(|| std::path::PathBuf::from("config.toml"));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, &content) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    // Best-effort reload into running state.
    if let Ok(new_cfg) = toml::from_str::<config::Config>(&content) {
        *st.cfg.lock().unwrap() = new_cfg.auth0;
    }
    Json(json!({
        "status": "ok",
        "path": path.display().to_string(),
    })).into_response()
}

async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

#[derive(Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn login(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let cfg = st.cfg.lock().unwrap().clone();
    if cfg.domain.is_empty() || cfg.client_id.is_empty() {
        return (StatusCode::SERVICE_UNAVAILABLE, "Auth0 not configured").into_response();
    }
    let state = Uuid::new_v4().to_string();
    st.pending.lock().unwrap().insert(state.clone(), now());
    let redirect_uri = format!("{}/callback", base_url(&headers));
    let mut url = url::Url::parse(&format!("https://{}/authorize", cfg.domain)).unwrap();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", &cfg.scopes)
        .append_pair("state", &state);
    if !cfg.audience.is_empty() {
        url.query_pairs_mut().append_pair("audience", &cfg.audience);
    }
    Redirect::temporary(url.as_str()).into_response()
}

async fn callback(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<CallbackParams>,
) -> Response {
    let cfg = st.cfg.lock().unwrap().clone();
    if let Some(err) = q.error {
        let desc = q.error_description.unwrap_or_default();
        return (StatusCode::BAD_REQUEST, format!("auth error: {err} {desc}")).into_response();
    }
    let code = match q.code {
        Some(c) => c,
        None => return (StatusCode::BAD_REQUEST, "missing code").into_response(),
    };
    let state = match q.state {
        Some(s) => s,
        None => return (StatusCode::BAD_REQUEST, "missing state").into_response(),
    };
    // verify state
    {
        let mut pending = st.pending.lock().unwrap();
        match pending.remove(&state) {
            Some(t) if now().saturating_sub(t) < 600 => {}
            _ => return (StatusCode::BAD_REQUEST, "invalid or expired state").into_response(),
        }
    }
    let redirect_uri = format!("{}/callback", base_url(&headers));
    let token_url = format!("https://{}/oauth/token", cfg.domain);
    let res = st
        .client
        .post(&token_url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
        ])
        .send()
        .await;

    let res = match res {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("token request failed: {e}")).into_response(),
    };
    if !res.status().is_success() {
        let body = res.text().await.unwrap_or_default();
        return (StatusCode::BAD_GATEWAY, format!("token exchange failed: {body}")).into_response();
    }
    let token_json: serde_json::Value = match res.json().await {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("bad token json: {e}")).into_response(),
    };
    let access_token = token_json.get("access_token").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let id_token = token_json.get("id_token").and_then(|v| v.as_str()).map(|s| s.to_string());

    // get userinfo
    let userinfo_url = format!("https://{}/userinfo", cfg.domain);
    let user = if !access_token.is_empty() {
        match st.client.get(&userinfo_url).bearer_auth(&access_token).send().await {
            Ok(r) if r.status().is_success() => r.json().await.unwrap_or_else(|_| json!({})),
            _ => json!({}),
        }
    } else {
        json!({})
    };

    // create session
    let sid = Uuid::new_v4().to_string();
    st.sessions.lock().unwrap().insert(
        sid.clone(),
        Session {
            user,
            access_token,
            id_token,
        },
    );
    let secure = is_secure(&headers);
    let cookie = if secure {
        format!("session={}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=86400", sid)
    } else {
        format!("session={}; HttpOnly; SameSite=Lax; Path=/; Max-Age=86400", sid)
    };
    let mut resp = Redirect::temporary("/").into_response();
    resp.headers_mut().insert(header::SET_COOKIE, cookie.parse().unwrap());
    resp
}

async fn logout(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let cfg = st.cfg.lock().unwrap().clone();
    // clear session
    if let Some(sid) = get_cookie(&headers, "session") {
        st.sessions.lock().unwrap().remove(&sid);
    }
    let return_to = base_url(&headers);
    let logout_url = format!(
        "https://{}/v2/logout?client_id={}&returnTo={}",
        cfg.domain,
        cfg.client_id,
        urlencoding_simple(&return_to)
    );
    let mut resp = if !cfg.domain.is_empty() {
        Redirect::temporary(&logout_url).into_response()
    } else {
        Redirect::temporary("/").into_response()
    };
    let clear = "session=; HttpOnly; Path=/; Max-Age=0";
    resp.headers_mut().insert(header::SET_COOKIE, clear.parse().unwrap());
    resp
}

fn urlencoding_simple(s: &str) -> String {
    // minimal percent-encoding for URL values
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

async fn me(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let sid = match get_cookie(&headers, "session") {
        Some(s) => s,
        None => return (StatusCode::UNAUTHORIZED, Json(json!({"authenticated": false}))).into_response(),
    };
    let sessions = st.sessions.lock().unwrap();
    match sessions.get(&sid) {
        Some(s) => Json(json!({"authenticated": true, "user": s.user, "has_access_token": !s.access_token.is_empty()})).into_response(),
        None => (StatusCode::UNAUTHORIZED, Json(json!({"authenticated": false}))).into_response(),
    }
}

async fn token(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let sid = match get_cookie(&headers, "session") {
        Some(s) => s,
        None => return (StatusCode::UNAUTHORIZED, Json(json!({"error": "not authenticated"}))).into_response(),
    };
    let sessions = st.sessions.lock().unwrap();
    match sessions.get(&sid) {
        Some(s) => Json(json!({"access_token": s.access_token, "id_token": s.id_token})).into_response(),
        None => (StatusCode::UNAUTHORIZED, Json(json!({"error": "not authenticated"}))).into_response(),
    }
}

#[tokio::main]
async fn main() {
    let cfg = config::Config::load();
    let host = cfg.server.host.clone();
    let config_path = config::Config::resolve_path();
    let (listener, port) = bind_listener(&host, cfg.server.port).await;
    let st = Arc::new(AppState {
        port,
        name: "auth0-demo",
        cfg: Mutex::new(cfg.auth0),
        config_path,
        client: reqwest::Client::new(),
        sessions: Mutex::new(HashMap::new()),
        pending: Mutex::new(HashMap::new()),
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/style.css", get(style_css))
        .route("/manifest.webmanifest", get(manifest))
        .route("/sw.js", get(sw))
        .route("/health", get(health))
        .route("/api/config", get(get_config).post(post_config))
        .route("/config", get(get_config).post(post_config))
        .route("/login", get(login))
        .route("/callback", get(callback))
        .route("/logout", get(logout))
        .route("/api/me", get(me))
        .route("/api/token", get(token))
        .fallback(not_found)
        .with_state(st)
        .layer(CompressionLayer::new());

    println!("auth0-demo listening on http://{host}:{port}");
    let listener = axum::serve(listener, app);
    tokio::select! {
        res = listener => {
            if let Err(e) = res {
                eprintln!("server error: {e}");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            println!("shutting down");
        }
    }
}
