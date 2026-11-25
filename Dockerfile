FROM rust:1-bookworm@sha256:ade39a710ef85f3506cffcfec3e8a24ba090badef1dff2fb798f71d878833fea AS source
WORKDIR /work
COPY Cargo.toml Cargo.lock ./
COPY src ./src

FROM source AS check
RUN rustup component add rustfmt clippy && cargo fetch --locked
COPY tests ./tests
RUN --network=none cargo fmt --check \
 && cargo clippy --offline --locked --all-targets -- -D warnings \
 && cargo test --offline --locked
