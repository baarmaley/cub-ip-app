use anyhow::Context;
use log::LevelFilter;
use serde::{de, Deserialize, Deserializer};
use std::time::Duration;

#[derive(Deserialize)]
pub struct Config {
    pub cub_ip_address: String,
    pub cub_ip_login: String,
    pub cub_ip_password: String,
    #[serde(deserialize_with = "deserialize_duration_from_str")]
    pub cub_ip_connection_timeout: Duration,
    #[serde(deserialize_with = "deserialize_duration_from_str")]
    pub cub_ip_read_timeout: Duration,
    #[serde(deserialize_with = "deserialize_duration_from_str")]
    pub cub_ip_write_timeout: Duration,
    #[serde(deserialize_with = "deserialize_duration_from_str")]
    pub cub_ip_request_timeout: Duration,
    pub mqtt_main_topic: String,
    pub mqtt_host: String,
    pub mqtt_name_client: String,
    #[serde(deserialize_with = "deserialize_level_filter_from_str")]
    pub log_level: LevelFilter,
}

fn deserialize_duration_from_str<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    parse_duration::parse(s.as_str()).map_err(de::Error::custom)
}

fn deserialize_level_filter_from_str<'de, D>(deserializer: D) -> Result<LevelFilter, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    match s.to_lowercase().as_str() {
        "off" => Ok(LevelFilter::Off),
        "error" => Ok(LevelFilter::Error),
        "warn" => Ok(LevelFilter::Warn),
        "info" => Ok(LevelFilter::Info),
        "debug" => Ok(LevelFilter::Debug),
        "trace" => Ok(LevelFilter::Trace),
        _ => Err(de::Error::custom("Unknown level filter")),
    }
}

impl Config {
    pub fn read_from_file(path: &str) -> anyhow::Result<Config> {
        let mut settings = config::Config::default();
        settings
            .merge(config::File::with_name(path))
            .with_context(|| format!("Config::read_from_file(): open file {}", path))?;

        settings
            .get("main")
            .with_context(|| "Config::read_from_file(): deserialize")
    }
}
