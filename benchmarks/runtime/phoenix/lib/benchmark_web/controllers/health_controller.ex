defmodule BenchmarkWeb.HealthController do
  use BenchmarkWeb, :controller

  def index(conn, _params) do
    text(conn, "ok")
  end
end
