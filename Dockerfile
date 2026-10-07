FROM ubuntu:24.04 AS builder

RUN apt-get update && apt-get install -y \
 build-essential \
 curl \
 pkg-config \
 libssl-dev \
 && rm -rf /var/lib/apt/lists/*

RUN curl https://sh.rustup.rs | bash -s -- -y --profile minimal
ENV PATH="/root/.cargo/bin:$PATH"

WORKDIR /src

# Dependencies on their own layer, so a source change doesn't recompile them.
COPY rust-toolchain Cargo.toml Cargo.lock build.rs ./
COPY migrations ./migrations
RUN mkdir src && touch src/lib.rs && echo 'fn main() {}' > src/main.rs && \
 cargo build --release && \
 rm -rf src target/release/.fingerprint/performance-service-*

COPY src ./src
RUN cargo build --release


FROM ubuntu:24.04

# curl is for the compose healthcheck.
RUN apt-get update && apt-get install -y --no-install-recommends \
 ca-certificates \
 curl \
 libssl3t64 \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /src

COPY --from=builder /src/target/release/performance-service ./target/release/performance-service

CMD ["./target/release/performance-service"]
