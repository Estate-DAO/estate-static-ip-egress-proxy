use hickory_resolver::TokioAsyncResolver;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

// Custom DNS resolver that wraps hickory-resolver
#[derive(Clone)]
pub struct HickoryDnsResolver {
    resolver: TokioAsyncResolver,
}

impl HickoryDnsResolver {
    pub fn new() -> Self {
        // Create custom resolver options with optimized caching
        let mut opts = hickory_resolver::config::ResolverOpts::default();
        opts.cache_size = 1024; // Increase cache size
        opts.use_hosts_file = true;
        opts.timeout = Duration::from_secs(3); // Reduce timeout from default
        opts.attempts = 2; // Reduce retry attempts

        let resolver =
            TokioAsyncResolver::tokio(hickory_resolver::config::ResolverConfig::default(), opts);

        HickoryDnsResolver { resolver }
    }
}
// Custom trait implementation for reqwest DNS resolution
impl reqwest::dns::Resolve for HickoryDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let resolver = self.resolver.clone();
        let host = name.as_str().to_string();

        Box::pin(async move {
            let start = Instant::now();
            debug!("inst_hickory_dns: Resolving hostname: {}", host);

            match resolver.lookup_ip(host.as_str()).await {
                Ok(lookup) => {
                    let addrs: Vec<SocketAddr> =
                        lookup.iter().map(|ip| SocketAddr::new(ip, 0)).collect();

                    let duration = start.elapsed();
                    info!(
                        "inst_hickory_dns: DNS resolution for {} took {:?}",
                        host, duration
                    );
                    debug!(
                        "inst_hickory_dns: Resolved {} to {} addresses",
                        host,
                        addrs.len()
                    );

                    Ok(Box::new(addrs.into_iter()) as Box<dyn Iterator<Item = SocketAddr> + Send>)
                }
                Err(e) => {
                    info!("inst_hickory_dns: Failed to resolve {}: {}", host, e);
                    Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("inst_hickory_dns: DNS resolution failed: {}", e),
                    )) as Box<dyn Error + Send + Sync>)
                }
            }
        })
    }
}
