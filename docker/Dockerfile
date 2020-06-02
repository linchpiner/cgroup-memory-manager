FROM rust:1.41.0 AS builder

WORKDIR /usr/src/cgroup-memory-manager
COPY . .
RUN cargo install --path .

FROM debian:buster-slim
COPY --from=builder /usr/local/cargo/bin/cgroup-memory-manager /usr/local/bin/cgroup-memory-manager
CMD ["cgroup-memory-manager"]

