use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub listener: ListenerConfig,
    pub tls: TlsConfig,
    pub admin: AdminConfig,
    pub plugins: PluginsConfig,
    pub upstream: Vec<UpstreamConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListenerConfig {
    pub addr: String,
    pub http_addr: String,
    #[serde(default = "default_workers")]
    pub workers: usize,
}

fn default_workers() -> usize { 0 }

#[derive(Debug, Clone, Deserialize)]
pub struct TlsConfig {
    pub cert: String,
    pub key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdminConfig {
    pub addr: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginsConfig {
    pub dir: String,
    pub pipeline: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamConfig {
    pub name: String,
    pub targets: Vec<String>,
    #[serde(default = "default_lb")]
    pub lb: String,
    #[serde(default = "default_health_path")]
    pub health_check_path: String,
    #[serde(default = "default_health_interval")]
    pub health_check_interval: u64,
}

fn default_lb() -> String { "round_robin".into() }
fn default_health_path() -> String { "/health".into() }
fn default_health_interval() -> u64 { 5 }

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&s)?;
        Ok(cfg)
    }
}
