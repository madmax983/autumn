FROM hexpm/elixir:1.17.3-erlang-27.1-debian-bookworm-20241016 AS builder
ENV MIX_ENV=prod
WORKDIR /app

RUN mix local.hex --force && mix local.rebar --force

COPY mix.exs mix.lock* ./
RUN mix deps.get --only prod && mix deps.compile

COPY config/config.exs config/prod.exs config/runtime.exs config/
COPY lib ./lib
RUN mix compile && mix release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 libncurses6 curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/_build/prod/rel/benchmark ./

ENV PHX_SERVER=true
ENV PORT=8080
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=5s --start-period=30s \
  CMD curl -sf http://localhost:${PORT}/health || exit 1

CMD ["/app/bin/benchmark", "start"]
