FROM rust:1.76 as build

RUN USER=gilbertnm cargo new --bin balance_bot
WORKDIR /balance_bot

COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml

RUN cargo build --release
RUN rm src/*.rs

COPY ./src ./src

RUN rm ./target/release/deps/balance_bot*
RUN cargo build --release

FROM rust:1.76

COPY --from=build /balance_bot/target/release/balance_bot .

CMD ["./balance_bot"]
