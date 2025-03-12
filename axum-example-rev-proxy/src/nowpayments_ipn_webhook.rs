use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha512;
use tracing::{error, info};
type HmacSha512 = Hmac<Sha512>;
use axum::extract::ConnectInfo;
use axum::extract::State;
use std::net::IpAddr;

use crate::app_state::AppState;
use crate::sort_json::sort_json;

// Define whitelist (could be a lazy_static or const once computed)
static NOWPAYMENTS_ALLOWED_IPS: &[&str] = &[
    "51.89.194.21",
    "51.75.77.69",
    "138.201.172.58",
    "65.21.158.36",
];

// todo see scratchpad_me.md for more security hardening
pub async fn nowpayments_webhook(
    ConnectInfo(remote_addr): ConnectInfo<std::net::SocketAddr>,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, &'static str) {
    let client_ip = remote_addr.ip();
    // Only allow if in whitelist
    let allowed = NOWPAYMENTS_ALLOWED_IPS
        .iter()
        .any(|&ip| client_ip == ip.parse::<IpAddr>().unwrap());

    if !allowed {
        tracing::warn!("Rejected webhook from unauthorized IP: {}", client_ip);
        return (StatusCode::FORBIDDEN, "Forbidden");
    }
    // 1. Extract signature from headers
    let signature = match headers.get("x-nowpayments-sig") {
        Some(sig) => sig,
        None => {
            error!("Missing x-nowpayments-sig header");
            return (StatusCode::BAD_REQUEST, "Signature missing");
        }
    };
    let signature = match signature.to_str() {
        Ok(s) => s,
        Err(_) => {
            error!("Invalid signature header format");
            return (StatusCode::BAD_REQUEST, "Invalid signature format");
        }
    };

    // 2. Parse JSON body
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(val) => val,
        Err(e) => {
            error!("Failed to parse JSON body: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "JSON parsing error");
        }
    };

    // 3. Sort JSON object
    let sorted_payload = sort_json(&payload);
    let payload_str = serde_json::to_string(&sorted_payload).unwrap_or_default();

    // 4. Compute HMAC-SHA512 signature
    let mut mac = HmacSha512::new_from_slice(state.env_var_config.ipn_secret.as_bytes())
        .expect("HMAC key creation failed");
    mac.update(payload_str.as_bytes());
    let computed_hmac = mac.finalize().into_bytes();
    let computed_hex = hex::encode(computed_hmac);

    // 5. Compare signatures
    if computed_hex.eq(signature) {
        info!("NowPayments webhook signature verified successfully");
        (StatusCode::OK, "OK")
    } else {
        error!(
            "Signature verification failed: expected {}, got {}",
            computed_hex, signature
        );
        (StatusCode::BAD_REQUEST, "Invalid signature")
    }
}
