# The SolidCraft PDM server. From the project folder:
#   docker build -f packaging/docker/Dockerfile -t solidcraft-pdm-server .
# Data (users, vaults) lives in the volume /data; the server listens on port 7380.

FROM rust:1.93-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p scpdm-server \
    && cp target/release/scpdm-server /usr/local/bin/

FROM debian:bookworm-slim
RUN useradd --system --uid 7380 --home-dir /data pdm \
    && mkdir /data && chown pdm /data
COPY --from=build /usr/local/bin/scpdm-server /usr/local/bin/
USER pdm
ENV SCPDM_SERVER_DATA=/data \
    SCPDM_SERVER_LISTEN=0.0.0.0:7380
VOLUME /data
EXPOSE 7380
ENTRYPOINT ["scpdm-server"]
CMD ["serve"]
