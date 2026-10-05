FROM docker.io/rust:1-slim-trixie as cargo-build
WORKDIR /usr/local/src/etherbalance

# Install dependencies
RUN apt-get update && apt-get install -y git libssl-dev pkg-config

# Copy and Build Code
COPY . .
RUN CARGO_PROFILE_RELEASE_DEBUG=1 cargo build --release

# Extract Binary
FROM docker.io/debian:trixie-slim

# Handle signal handlers properly
RUN apt-get update && apt-get install -y ca-certificates tini && apt-get clean
COPY --from=cargo-build /usr/local/src/etherbalance/target/release/etherbalance /usr/local/bin/etherbalance

ENTRYPOINT ["/usr/bin/tini", "etherbalance", "--"]
