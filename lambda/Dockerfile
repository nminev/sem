# Stage 1a: install Rust + cargo-chef
FROM amazonlinux:2023 AS chef

RUN dnf install -y gcc openssl-devel libgit2-devel cmake make git tar gzip

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:$PATH"

RUN cargo install cargo-chef --locked
WORKDIR /sem

# Stage 1b: build a dependency recipe from Cargo.toml/Cargo.lock only.
# This step's input is just the manifests, so any source change leaves it untouched.
FROM chef AS planner
COPY crates ./crates
WORKDIR /sem/crates
RUN cargo chef prepare --recipe-path recipe.json

# Stage 1c: cook (compile) all dependencies once. Cached as long as recipe.json is unchanged.
FROM chef AS deps
COPY --from=planner /sem/crates/recipe.json crates/recipe.json
WORKDIR /sem/crates
RUN cargo chef cook --release --recipe-path recipe.json

# Stage 1d: copy real source, rebuild only our crates against the cached deps.
# WORKDIR reset is load-bearing: without it, COPY lands source under
# /sem/crates/crates and cargo builds cargo-chef's placeholder root instead.
FROM deps AS builder
WORKDIR /sem
COPY crates ./crates
WORKDIR /sem/crates
RUN cargo build --release -p sem-cli

# Stage 2: Lambda runtime
FROM public.ecr.aws/lambda/python:3.12

RUN dnf install -y libgit2

COPY --from=builder /sem/crates/target/release/sem /var/task/sem
RUN chmod +x /var/task/sem

COPY lambda/handler.py /var/task/

CMD ["handler.lambda_handler"]
