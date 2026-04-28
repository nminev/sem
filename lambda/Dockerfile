# Stage 1: build sem from this repo
FROM amazonlinux:2023 AS builder

RUN dnf install -y gcc openssl-devel libgit2-devel cmake make git tar gzip

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:$PATH"

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
