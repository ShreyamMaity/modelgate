FROM rust:1-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
RUN cargo build --release --locked

FROM debian:trixie-slim
RUN useradd -u 10001 -M app && mkdir /models
COPY --from=build /src/target/release/pii-ner /usr/local/bin/pii-ner
USER app
ENV LISTEN=0.0.0.0:8090 NER_MODEL=/models/model.onnx NER_TOKENIZER=/models/tokenizer.json
EXPOSE 8090
HEALTHCHECK --interval=15s --timeout=3s --retries=3 \
  CMD ["pii-ner", "health"]
ENTRYPOINT ["pii-ner"]
