FROM ruby:3.3.6-slim-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential libpq-dev git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Gemfile Gemfile.lock* ./
RUN bundle install --without development test --jobs 4 --retry 3

COPY . .
RUN bundle exec bootsnap precompile --gemfile app/ lib/

FROM ruby:3.3.6-slim-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
    libpq5 curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /usr/local/bundle /usr/local/bundle
COPY --from=builder /app /app

ENV RAILS_ENV=production
ENV RAILS_LOG_TO_STDOUT=true
ENV RAILS_SERVE_STATIC_FILES=true
ENV PORT=8080
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=5s --start-period=30s \
  CMD curl -sf http://localhost:${PORT}/health || exit 1

CMD ["bundle", "exec", "puma", "-C", "config/puma.rb"]
