FROM rust:latest as build

RUN USER=root cargo new --bin performance-service
WORKDIR /performance-service

COPY ./Cargo.toml ./Cargo.toml
COPY ./build.rs ./build.rs

RUN cargo build --release

COPY ./src ./src

#RUN rm ./target/release/deps/performance-service*
RUN cargo build --release

FROM debian:buster-slim

COPY --from=build /performance-service/target/release/performance-service .
CMD ["./performance-service"]
