FROM ubuntu:24.04

RUN apt-get update && apt-get install -y \
 build-essential \
 curl \
 pkg-config \
 libssl-dev

RUN curl https://sh.rustup.rs | bash -s -- -y

WORKDIR /src

COPY . /src

RUN export PATH="$HOME/.cargo/bin:$PATH" && \
 cargo build --release

CMD ["./target/release/performance-service"]