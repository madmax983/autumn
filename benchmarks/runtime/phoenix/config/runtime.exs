import Config

if config_env() == :prod do
  database_url = System.get_env("DATABASE_URL") ||
    raise "DATABASE_URL environment variable is not set"

  config :benchmark, Benchmark.Repo,
    url: database_url,
    pool_size: String.to_integer(System.get_env("POOL_SIZE") || "20")

  secret_key_base = System.get_env("SECRET_KEY_BASE") ||
    raise "SECRET_KEY_BASE environment variable is not set"

  port = String.to_integer(System.get_env("PORT") || "8080")

  config :benchmark, BenchmarkWeb.Endpoint,
    http: [ip: {0, 0, 0, 0}, port: port],
    secret_key_base: secret_key_base,
    server: true
end
