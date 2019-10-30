FROM rust:1.38

WORKDIR /usr/src/img_api
COPY . .

RUN cargo build --release

WORKDIR /usr/src/img_api/target/release

CMD /usr/src/img_api/target/release/img_api