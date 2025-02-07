use serde::Deserialize;
use std::env::VarError;
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub struct EnvVarConfig {
    pub ipn_secret: String,
}

impl EnvVarConfig {
    pub fn try_from_env() -> Self {
        let value = Self {
            // todo add secret when available in gh actions
            ipn_secret: env_w_default("NOWPAYMENTS_IPN_SECRET", "dummy-secret-for-now").unwrap(),
        };

        // println!("{value:#?}");
        value
    }
}

/// Application state shared by handlers.
#[derive(Clone)]
pub struct AppState {
    pub client: reqwest::Client,
    pub env_var_config: EnvVarConfig,
}

impl AppState {
    pub async fn build(client: reqwest::Client) -> Self {
        Self {
            client,
            env_var_config: EnvVarConfig::try_from_env(),
        }
    }
}

//
// PRIVATE METHODS
//

fn env_w_default(key: &str, default: &str) -> Result<String, EstateEnvConfigError> {
    match std::env::var(key) {
        Ok(val) => Ok(val),
        Err(VarError::NotPresent) => Ok(default.to_string()),
        Err(e) => Err(EstateEnvConfigError::EnvVarError(format!(
            "missing {key}: {e}"
        ))),
    }
}

fn env_wo_default(key: &str) -> Result<Option<String>, EstateEnvConfigError> {
    match std::env::var(key) {
        Ok(val) => Ok(Some(val)),
        Err(VarError::NotPresent) => Ok(None),
        Err(e) => Err(EstateEnvConfigError::EnvVarError(format!("{key}: {e}"))),
    }
}

fn env_or_panic(key: &str) -> String {
    match std::env::var(key) {
        Ok(val) => val,
        Err(e) => panic!("missing {key}: {e}"),
    }
}

#[derive(Debug, Error, Clone)]
pub enum EstateEnvConfigError {
    #[error("Failed to get Estate Environment. Did you set environment vairables?")]
    EnvError,
    #[error("Config Error: {0}")]
    EnvVarError(String),
}
