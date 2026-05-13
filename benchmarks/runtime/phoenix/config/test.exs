import Config

config :benchmark, Benchmark.Repo,
  username: "benchmark",
  password: "benchmark",
  hostname: "localhost",
  database: "benchmark_test",
  pool: Ecto.Adapters.SQL.Sandbox,
  pool_size: 5

config :benchmark, BenchmarkWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 4002],
  secret_key_base: "test_secret_key_base_0000000000000000000000000000000000000000000000"

config :logger, level: :warning
