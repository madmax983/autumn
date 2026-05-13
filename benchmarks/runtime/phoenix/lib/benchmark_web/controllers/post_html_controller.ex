defmodule BenchmarkWeb.PostHtmlController do
  use BenchmarkWeb, :controller
  alias Benchmark.{Post, Repo}

  def index(conn, _params) do
    posts = Post.recent(Repo)
    render(conn, :index, posts: posts)
  end

  def show(conn, %{"id" => id}) do
    case Repo.get(Post, id) do
      nil  -> conn |> put_status(404) |> text("not found")
      post -> render(conn, :show, post: post)
    end
  end
end
