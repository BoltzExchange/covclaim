FROM rust:1.78

WORKDIR /app

COPY ./ ./

RUN cargo build --release

EXPOSE 1234

CMD ["./target/release/covclaim"]