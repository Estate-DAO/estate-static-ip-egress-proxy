use axum::body::to_bytes;
use axum::extract::Path;
use axum::routing::{any, post, get};
use axum::{
    body::Body,
    extract::{Request, State},
    http::uri::Uri,
    response::Response,
    Router,
};
use hyper::{header, StatusCode};
use reqwest;
use serde::Deserialize;
use tower_http::trace::TraceLayer;
use tracing::{error, info, debug, warn};
use std::sync::Arc;
use std::time::Duration;

mod app_state;
mod nowpayments_ipn_webhook;
mod sort_json;
mod dns_resolver;

use app_state::AppState;
use nowpayments_ipn_webhook::nowpayments_webhook;
use dns_resolver::HickoryDnsResolver;

type Client = reqwest::Client;

use std::time::Instant;

/// Struct to deserialize path parameters.
/// - `env`: Represents the environment (`test` or `prod`).
/// - `wildcard_path`: Represents the remaining path after the environment prefix.
#[derive(Deserialize)]
struct PathParams {
    env: String,
    wildcard_path: String,
}

const MAX_BODY_SIZE: usize = 8 * 1024 * 1024; // 8 MB

#[tokio::main]
async fn main() {
    // Initialize tracing for logging
    tracing_subscriber::fmt::init();

    let trace_layer = TraceLayer::new_for_http().make_span_with(|request: &axum::extract::Request<Body>| {
        let uri = request.uri().to_string();
        tracing::info_span!("proxifier_http_request", method = ?request.method(), uri)
    });

    // Create our custom DNS resolver - for dns caching
    let dns_resolver = HickoryDnsResolver::new();
    
    // Build the reqwest client with our custom resolver and more detailed settings
    let client = Client::builder()
        .dns_resolver(Arc::new(dns_resolver))
        .connection_verbose(true) // Enable verbose connection metrics
        .timeout(Duration::from_secs(30)) // Overall request timeout
        .connect_timeout(Duration::from_secs(10)) // Connection timeout
        .pool_idle_timeout(Duration::from_secs(90)) // Keep connections alive
        .pool_max_idle_per_host(10) // Maximum idle connections per host
        .https_only(false) // Allow both HTTP and HTTPS
        .tcp_keepalive(Duration::from_secs(60)) // TCP keepalive
        .build()
        .expect("Failed to create reqwest client");

    // Build AppState from app_state.rs - now includes metrics
    let app_state = AppState::build(client).await;

    let app = Router::new()
        .route("/nowpayments-webhook", post(nowpayments_webhook))
        .route("/metrics", get(get_metrics))
        .route("/health", get(health_check))
        .route("/{env}/{*wildcard_path}", any(handler))
        .with_state(app_state)
        .layer(trace_layer);

    let port = std::env::var("AXUM_PROXY_PORT")
        .map(|s| s.parse::<u16>().expect("AXUM_PROXY_PORT must be a u16"))
        .unwrap_or(80);

    // Create listener for IPv6
    let ipv6_listener = tokio::net::TcpListener::bind(format!("[::]:{}", port)).await.unwrap();
    tracing::info!("Listening on IPv6 {}", ipv6_listener.local_addr().unwrap());

    axum::serve(ipv6_listener, app).await.unwrap();
}

/// Health check endpoint
async fn health_check() -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from("OK"))
        .unwrap()
}

/// Endpoint to expose collected metrics
async fn get_metrics(State(state): State<AppState>) -> Response<Body> {
    let metrics = state.metrics.lock().unwrap();
    
    let uptime = metrics.start_time.elapsed().unwrap_or_default();
    let uptime_secs = uptime.as_secs();
    
    let mut response_body = format!(
        "# PROXY METRICS\n\n## Summary\n\n\
        - Uptime: {}d {}h {}m {}s\n\
        - Total Requests: {}\n\
        - Successful Requests: {}\n\
        - Failed Requests: {}\n\
        - Avg Response Time: {:.2}ms\n\
        - Slowest Request: {}ms ({})\n\n",
        uptime_secs / 86400, (uptime_secs % 86400) / 3600, (uptime_secs % 3600) / 60, uptime_secs % 60,
        metrics.total_requests,
        metrics.successful_requests,
        metrics.failed_requests,
        metrics.avg_request_time_ms,
        metrics.slowest_request_time_ms,
        metrics.slowest_request_path
    );
    
    response_body.push_str("## Environment Usage\n\n");
    for (env, count) in &metrics.env_requests {
        response_body.push_str(&format!("- {}: {}\n", env, count));
    }
    
    response_body.push_str("\n## Status Codes\n\n");
    for (status, count) in &metrics.requests_by_status {
        response_body.push_str(&format!("- {}: {}\n", status, count));
    }
    
    response_body.push_str("\n## Errors\n\n");
    response_body.push_str(&format!(
        "- Connection Errors: {}\n\
        - Timeout Errors: {}\n\
        - DNS Errors: {}\n",
        metrics.connection_errors,
        metrics.timeout_errors,
        metrics.dns_errors
    ));
    
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(response_body))
        .unwrap()
}

async fn handler(
    State(state): State<AppState>,
    Path(PathParams { env, wildcard_path }): Path<PathParams>,
    req: Request,
) -> Result<Response, StatusCode> {
    let total_start = Instant::now();
    let path = format!("/{}", wildcard_path);

    // Determine the target_base URL based on the environment
    let target_base = match env.as_str() {
        "test" => "http://test.services.travelomatix.com",
        "prod" => "https://prod.services.travelomatix.com",
        _ => {
            error!("Invalid environment: {}", env);
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    // Construct the new path by removing the `/test` or `/prod` prefix
    let new_path = format!("/{}", wildcard_path);

    let query = req
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let uri = format!("{}{}{}", target_base, new_path, query);
    info!("Forwarding to URI: {}", uri);

    // Parse target URI to extract host
    let target_uri = uri.parse::<Uri>().map_err(|e| {
        error!("Failed to parse target URI: {}", e);
        // Record DNS error
        if let Ok(mut metrics) = state.metrics.lock() {
            metrics.record_error("dns");
        }
        StatusCode::BAD_GATEWAY
    })?;

    // adjust headers
    let target_host = target_uri.host().ok_or_else(|| {
        error!("Missing host in target URI");
        StatusCode::BAD_GATEWAY
    })?;

    let mut headers = req.headers().clone();
    headers.remove(header::HOST);
    headers.insert(
        header::HOST,
        header::HeaderValue::from_str(target_host).map_err(|_| StatusCode::BAD_GATEWAY)?,
    );

    // Log request details for debugging
    debug!("Forwarding request to {}", uri);
    debug!("Method: {:?}", req.method());
    debug!("Headers: {:?}", headers);

    // Build outbound request
    let client = &state.client;
    let mut request_builder = client.request(req.method().clone(), &uri).headers(headers);

    // Forward body if present
    let network_start = Instant::now();
    let maybe_body = to_bytes(req.into_body(), MAX_BODY_SIZE).await;
    let body_read_time = network_start.elapsed().as_millis() as u64;

    debug!("Time to read request body: {}ms", body_read_time);
    
    if let Ok(bytes) = maybe_body {
        debug!("Request body size: {} bytes", bytes.len());
        request_builder = request_builder.body(bytes);
    }

    // Send the request and time it
    let network_time_start = Instant::now();
    let response_result = request_builder.send().await;
    let network_time = network_time_start.elapsed().as_millis() as u64;
    debug!("Network time: {}ms", network_time);

    let response = match response_result {
        Ok(resp) => resp,
        Err(e) => {
            // Detailed error logging and metrics
            let error_type = if e.is_timeout() {
                warn!("Request timed out: {}", e);
                "timeout"
            } else if e.is_connect() {
                warn!("Connection error: {}", e);
                "connection"
            } else {
                warn!("Request failed: {}", e);
                "other"
            };
            
            // Record the error
            if let Ok(mut metrics) = state.metrics.lock() {
                metrics.record_error(error_type);
            }
            
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

    // Handling the response
    let status = response.status();
    let headers = response.headers().clone();
    
    // Log response headers for debugging
    debug!("Response Status: {}", status);
    debug!("Response Headers: {:?}", headers);
    
    // Read response body and time it
    let body_time_start = Instant::now();
    let body_bytes_result = response.bytes().await;
    let body_time = body_time_start.elapsed().as_millis() as u64;
    debug!("Time to read response body: {}ms", body_time);
    
    let body_bytes = match body_bytes_result {
        Ok(bytes) => bytes,
        Err(e) => {
            error!("Failed to read response body: {}", e);
            
            // Record the error
            if let Ok(mut metrics) = state.metrics.lock() {
                metrics.record_error("connection");
            }
            
            return Err(StatusCode::BAD_GATEWAY);
        }
    };
    
    let body_size = body_bytes.len();
    debug!("Response body size: {} bytes", body_size);

    // Calculate total request time
    let total_time = total_start.elapsed().as_millis() as u64;
    info!("Total request time: {}ms (network: {}ms, body: {}ms)", 
         total_time, network_time, body_time);

    // Record metrics for this request
    if let Ok(mut metrics) = state.metrics.lock() {
        metrics.record_request(
            &path,
            &env,
            status.as_u16(),
            total_time,
            body_size
        );
    }

    // If the `debug_response` feature is enabled, we decode, log, and optionally re-encode.
    // Otherwise, we forward as-is.
    #[cfg(feature = "debug_response")]
    {
        for (key, value) in headers.iter() {
            info!("Response Header: {}: {:?}", key, value);
        }

        info!("`debug_response` feature is enabled: decoding and re-encoding the response.");

        // Check if the response is gzip-compressed
        let is_gzipped = headers
            .get(header::CONTENT_ENCODING)
            .map_or(false, |val| val == "gzip");

        // Decode the body into a string
        let decoded_data = if is_gzipped {
            let mut decoder = GzDecoder::new(&body_bytes[..]);
            let mut decoded_data = Vec::new();
            decoder.read_to_end(&mut decoded_data).map_err(|e| {
                error!("Failed to decode gzipped response: {}", e);
                StatusCode::BAD_GATEWAY
            })?;
            decoded_data
        } else {
            body_bytes.to_vec()
        };

        let body_string = String::from_utf8_lossy(&decoded_data).into_owned();

        // Log the decoded response
        info!("Decoded Response Body: {:?}", body_string);

        let final_body_bytes = body_string.into_bytes();

        let body_len = final_body_bytes.len();
        let mut new_response = Response::new(Body::from(final_body_bytes));
        *new_response.status_mut() = status;
        *new_response.headers_mut() = headers.clone();

        // Make sure we set the correct headers
        new_response.headers_mut().remove(header::TRANSFER_ENCODING);
        new_response.headers_mut().remove(header::CONNECTION);
        new_response.headers_mut().insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from(body_len as u64),
        );

        return Ok(new_response);
    }

    // If `debug_response` is NOT enabled, forward everything as-is.
    #[cfg(not(feature = "debug_response"))]
    {
        debug!("`debug_response` feature is disabled: forwarding response as-is.");

        let body_len = body_bytes.len();
        let mut new_response = Response::new(Body::from(body_bytes));
        *new_response.status_mut() = status;
        *new_response.headers_mut() = headers;

        new_response.headers_mut().remove(header::TRANSFER_ENCODING);
        new_response.headers_mut().remove(header::CONNECTION);
        new_response.headers_mut().insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from(body_len as u64),
        );

        Ok(new_response)
    }
}
