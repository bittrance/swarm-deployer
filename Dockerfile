FROM rust:1.42-slim-stretch AS buildenv

RUN apt-get update && apt-get install --assume-yes pkg-config openssl libssl-dev
WORKDIR /usr/src/myapp
COPY . .
RUN cargo install --path .

FROM debian:stretch-slim

RUN apt-get update && apt-get install --assume-yes openssl ca-certificates
COPY --from=buildenv /usr/src/myapp/target/release/swarm-ecr-deployer /swarm-ecr-deployer

ENTRYPOINT ["/swarm-ecr-deployer"]
