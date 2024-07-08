FROM rust:1.79 AS builder

WORKDIR /app

COPY ./ ./

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
  apt-get upgrade && \
  apt-get install -y libsqlite3-0 libpq5 ca-certificates && \
  apt-get clean all && \
  rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/covclaim /usr/local/bin/covclaim
COPY --from=builder /app/.env /.env

EXPOSE 1234

CMD ["/usr/local/bin/covclaim"]
