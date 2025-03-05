# Stage 1: Build the Rust application
FROM rust:1.84.1-slim-bullseye as builder


# Install musl-tools to enable linking against musl
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools

# Set the environment variable to force static linking with OpenSSL
ENV OPENSSL_STATIC=1

# Add the musl target to Rust (ensuring a statically linked binary)
RUN rustup target add x86_64-unknown-linux-musl

# Set the working directory inside the container to the project folder.
WORKDIR /usr/src/app

# Copy Cargo manifests to leverage Docker’s layer cache.
COPY axum-example-rev-proxy/Cargo.toml axum-example-rev-proxy/Cargo.lock ./

# Copy the remaining source code.
COPY axum-example-rev-proxy/ .

# Build the application in release mode using the vendored feature.
RUN cargo build --release --target x86_64-unknown-linux-musl 



# Stage 2: Create a minimal runtime image
FROM debian:buster-slim
RUN apt-get update \
  && apt-get install -y ca-certificates \
  && rm -rf /var/lib/apt/lists/*

# Copy the built binary from the builder stage.
COPY --from=builder /usr/src/app/target/x86_64-unknown-linux-musl/release/axum-example-rev-proxy /usr/local/bin/axum-example-rev-proxy

# Set the RUST_LOG environment variable for logging
ENV RUST_LOG=debug

# Expose the port your Axum server listens on 
EXPOSE 80
# EXPOSE 8001

# Run the Axum server.
CMD ["axum-example-rev-proxy"]
