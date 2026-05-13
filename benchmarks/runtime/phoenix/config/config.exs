import Config

config :benchmark, BenchmarkWeb.Endpoint,
  url: [host: "localhost"],
  render_errors: [formats: [html: BenchmarkWeb.ErrorHTML, json: BenchmarkWeb.ErrorJSON], layout: false],
  pubsub_server: Benchmark.PubSub,
  live_view: [signing_salt: "benchmark_salt"]

config :benchmark, Benchmark.Repo,
  adapter: Ecto.Adapters.Postgres,
  pool_size: 20

config :logger, level: :warning

import_config "#{config_env()}.exs"
