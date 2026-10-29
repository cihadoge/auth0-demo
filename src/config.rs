//! Config: --config flag → $AUTH0_CONFIG → ./config.toml → /etc/auth0/config.toml

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Debug, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub server: ServerCfg,
    #[serde(default)]
    pub auth0: Auth0Cfg,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ServerCfg {
    #[serde(default = "d_host")]
    pub host: String,
    /// 0 = pick next available port ≥ 69.
    #[serde(default)]
    pub port: u16,
}

fn d_host() -> String {
    "0.0.0.0".into()
}

impl Default for ServerCfg {
    fn default() -> Self {
        Self { host: d_host(), port: 0 }
    }
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct Auth0Cfg {
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub audience: String,
    #[serde(default = "d_scopes")]
    pub scopes: String,
}

fn d_scopes() -> String {
    "openid profile email".into()
}

impl Config {
    pub fn load() -> Self {
        let path = Self::resolve_path();
        let mut cfg: Config = path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        // env overrides
        if let Ok(v) = std::env::var("AUTH0_DOMAIN") {
            cfg.auth0.domain = v;
        }
        if let Ok(v) = std::env::var("AUTH0_CLIENT_ID") {
            cfg.auth0.client_id = v;
        }
        if let Ok(v) = std::env::var("AUTH0_CLIENT_SECRET") {
            cfg.auth0.client_secret = v;
        }
        if let Ok(v) = std::env::var("AUTH0_AUDIENCE") {
            cfg.auth0.audience = v;
        }
        if let Ok(v) = std::env::var("AUTH0_SCOPES") {
            cfg.auth0.scopes = v;
        }
        cfg
    }

    fn resolve_path() -> Option<PathBuf> {
        if let Some(p) = std::env::args().skip_while(|a| a != "--config").nth(1) {
            return Some(PathBuf::from(p));
        }
        if let Ok(p) = std::env::var("AUTH0_CONFIG") {
            return Some(PathBuf::from(p));
        }
        let local = PathBuf::from("config.toml");
        if local.is_file() {
            return Some(local);
        }
        let system = PathBuf::from("/etc/auth0/config.toml");
        if system.is_file() {
            return Some(system);
        }
        None
    }
}

#[allow(dead_code)]
fn _assert_send(_: &Path) {}
