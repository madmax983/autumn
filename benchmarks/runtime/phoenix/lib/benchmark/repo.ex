defmodule Benchmark.Repo do
  use Ecto.Repo,
    otp_app: :benchmark,
    adapter: Ecto.Adapters.Postgres
end
