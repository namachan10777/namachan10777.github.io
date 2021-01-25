FROM rust:1.49-alpine

RUN apk update && \
    apk add curl git jq

RUN rustup component add clippy-preview rustfmt

ENTRYPOINT ["/bin/sh"]
