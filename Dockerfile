FROM lukemathwalker/cargo-chef:0.1.61-rust-1.70-slim-buster AS planner
WORKDIR /plan

COPY ./src ./src
COPY ./proto ./proto
COPY ./build.rs ./build.rs
COPY ./Cargo.lock .
COPY ./Cargo.toml .

RUN cargo chef prepare --recipe-path recipe.json

FROM lukemathwalker/cargo-chef:0.1.61-rust-1.70-buster AS builder

WORKDIR /build
RUN apt-get update && apt-get install cmake -y
RUN curl -o protoc.zip -L https://github.com/protocolbuffers/protobuf/releases/download/v21.4/protoc-21.4-linux-x86_64.zip
RUN unzip protoc.zip && rm -rf readme.txt && mv bin/protoc /usr/bin/protoc && rm -rf bin && protoc --version && mkdir -p /usr/include/google/protobuf/ && mv include/google/protobuf/* /usr/include/google/protobuf/ && rm -rf include

COPY --from=planner /plan/recipe.json recipe.json

RUN cargo chef cook --release --recipe-path recipe.json -p lvp

COPY ./src ./src
COPY ./proto ./proto
COPY ./build.rs ./build.rs
COPY ./Cargo.lock .
COPY ./Cargo.toml .

RUN cargo build --release -p lvp && mv /build/target/release/lvp /build/target/lvp

FROM debian:buster-slim
WORKDIR /runtime

RUN apt-get update && apt-get install libssl1.1 ca-certificates xfsprogs strace -y && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/lvp /runtime/lvp

ENTRYPOINT ["/runtime/lvp"]