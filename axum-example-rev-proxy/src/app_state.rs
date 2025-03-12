use serde::Deserialize;
use std::env::VarError;
use thiserror::Error;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::time::{SystemTime, Instant};
use serde::Serialize;

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

/// Metrics structure for tracking proxy performance
#[derive(Debug, Clone, Serialize)]
pub struct RequestMetrics {
    pub total_requests: usize,
    pub successful_requests: usize,
    pub failed_requests: usize,
    pub total_request_time_ms: u64,
    pub avg_request_time_ms: f64,
    pub requests_by_path: HashMap<String, usize>,
    pub requests_by_status: HashMap<u16, usize>,
    pub response_sizes: HashMap<String, usize>,
    pub slowest_request_time_ms: u64,
    pub slowest_request_path: String,
    pub connection_errors: usize,
    pub timeout_errors: usize,
    pub dns_errors: usize,
    pub env_requests: HashMap<String, usize>,
    pub start_time: SystemTime,
}

impl Default for RequestMetrics {
    fn default() -> Self {
        RequestMetrics {
            total_requests: 0,
            successful_requests: 0,
            failed_requests: 0,
            total_request_time_ms: 0,
            avg_request_time_ms: 0.0,
            requests_by_path: Default::default(),
            requests_by_status: Default::default(),
            response_sizes: Default::default(),
            slowest_request_time_ms: 0,
            slowest_request_path: Default::default(),
            connection_errors: 0,
            timeout_errors: 0,
            dns_errors: 0,
            env_requests: Default::default(),
            start_time: SystemTime::now(),
        }
    }
}

impl RequestMetrics {

    pub fn record_request(&mut self, path: &str, env: &str, status: u16, duration_ms: u64, response_size: usize) {
        self.total_requests += 1;
        
        // Record by path
        *self.requests_by_path.entry(path.to_string()).or_insert(0) += 1;
        
        // Record by environment
        *self.env_requests.entry(env.to_string()).or_insert(0) += 1;
        
        // Record by status code
        *self.requests_by_status.entry(status).or_insert(0) += 1;
        
        // Record response size
        self.response_sizes.insert(path.to_string(), response_size);
        
        // Track if successful or failed
        if status >= 200 && status < 400 {
            self.successful_requests += 1;
        } else {
            self.failed_requests += 1;
        }
        
        // Update timing metrics
        self.total_request_time_ms += duration_ms;
        self.avg_request_time_ms = self.total_request_time_ms as f64 / self.total_requests as f64;
        
        // Track slowest request
        if duration_ms > self.slowest_request_time_ms {
            self.slowest_request_time_ms = duration_ms;
            self.slowest_request_path = path.to_string();
        }
    }

    pub fn record_error(&mut self, error_type: &str) {
        match error_type {
            "connection" => self.connection_errors += 1,
            "timeout" => self.timeout_errors += 1,
            "dns" => self.dns_errors += 1,
            _ => {}
        }
    }
}

/// Application state shared by handlers.
#[derive(Clone)]
pub struct AppState {
    pub client: reqwest::Client,
    pub env_var_config: EnvVarConfig,
    pub metrics: Arc<Mutex<RequestMetrics>>,
}

impl AppState {
    pub async fn build(client: reqwest::Client) -> Self {
        Self {
            client,
            env_var_config: EnvVarConfig::try_from_env(),
            metrics: Arc::new(Mutex::new(RequestMetrics::default())),
        }
    }
}

// // PRIVATE METHODS //

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