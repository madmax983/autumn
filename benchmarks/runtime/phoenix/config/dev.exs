import Config

config :benchmark, Benchmark.Repo,
  username: "benchmark",
  password: "benchmark",
  hostname: "localhost",
  database: "benchmark",
  stacktrace: true,
  show_sensitive_data_on_connection_error: true,
  pool_size: 10

config :benchmark, BenchmarkWeb.Endpoint,
  http: [ip: {127, 0, 0, 1}, port: 4000],
  check_origin: false,
  code_reloader: true,
  debug_errors: true,
  secret_key_base: "dev_secret_key_base_0000000000000000000000000000000000000000000000"

config :logger, :console,
  format: "[$level] $message\n",
  metadata: [:request_id]
