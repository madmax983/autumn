defmodule Benchmark.Application do
  use Application

  @impl true
  def start(_type, _args) do
    children = [
      Benchmark.Repo,
      {Phoenix.PubSub, name: Benchmark.PubSub},
      BenchmarkWeb.Endpoint
    ]

    opts = [strategy: :one_for_one, name: Benchmark.Supervisor]
    Supervisor.start_link(children, opts)
  end

  @impl true
  def config_change(changed, _new, removed) do
    BenchmarkWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
