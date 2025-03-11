use axum::body::to_bytes;
use axum::extract::Path;
use axum::routing::{any, post};
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
use tracing::{error, info};
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

use std::time::{Instant};
use serde::Serialize;

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

    let trace_layer =
        TraceLayer::new_for_http().make_span_with(|request: &axum::extract::Request<_>| {
            let uri = request.uri().to_string();
            tracing::info_span!("proxifier_http_request", method = ?request.method(), uri)
        });

    // Create our custom DNS resolver - for dns caching
    // let dns_resolver = HickoryDnsResolver::new();
    
    // Build the reqwest client with our custom resolver
    let client = Client::builder()
        // .dns_resolver(Arc::new(dns_resolver))
        .connection_verbose(true) // Enable verbose connection metrics
        // .timeout(Duration::from_secs(10)) // Overall request timeout
        .build()
        .expect("Failed to create reqwest client");

    let app_state = AppState::build(client).await;

    let app = Router::new()
        .route("/nowpayments-webhook", post(nowpayments_webhook))
        .route("/{env}/{*wildcard_path}", any(handler))
        // NOWPayments webhook route.
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

async fn handler(
    State(app_state): State<AppState>,
    Path(PathParams { env, wildcard_path }): Path<PathParams>,
    req: Request,
) -> Result<Response, StatusCode> {
    let total_start = Instant::now();

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
        StatusCode::BAD_GATEWAY
    })?;

    //
    // adjust headers
    //

    let target_host = target_uri.host().ok_or(StatusCode::BAD_GATEWAY)?;

    let mut headers = req.headers().clone();
    headers.remove(header::HOST);
    headers.insert(
        header::HOST,
        header::HeaderValue::from_str(target_host).map_err(|_| StatusCode::BAD_GATEWAY)?,
    );

    // Build outbound request
    let client = app_state.client;
    let mut request_builder = client.request(req.method().clone(), &uri).headers(headers);

    // Forward body if present
    if let Ok(bytes) = to_bytes(req.into_body(), MAX_BODY_SIZE).await {
        request_builder = request_builder.body(bytes);
    }

    let response = request_builder.send().await.map_err(|e| {
        error!("Request failed: {}", e);
        StatusCode::BAD_GATEWAY
    })?;

    //
    // == Handling the response ==
    //
    let status = response.status();
    let headers = response.headers().clone();
    let body_bytes = response.bytes().await.map_err(|e| {
        error!("Failed to read response body: {}", e);
        StatusCode::BAD_GATEWAY
    })?;

    info!("Response Status: {}", status);

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

        // Optionally re-encode if it was originally gzipped
        // let final_body_bytes = if is_gzipped {
        //     let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        //     encoder.write_all(body_string.as_bytes()).map_err(|e| {
        //         error!("Failed to re-encode gzipped response: {}", e);
        //         StatusCode::BAD_GATEWAY
        //     })?;
        //     match encoder.finish() {
        //         Ok(b) => b,
        //         Err(e) => {
        //             error!("Failed to finish gzip encoder: {}", e);
        //             return Err(StatusCode::BAD_GATEWAY);
        //         }
        //     }
        // } else {
        //     // If it wasn't gzipped, just forward the plain bytes
        //     body_string.into_bytes()
        // };

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
        info!("`debug_response` feature is disabled: forwarding response as-is.");

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
