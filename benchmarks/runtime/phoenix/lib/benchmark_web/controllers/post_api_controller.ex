defmodule BenchmarkWeb.PostApiController do
  use BenchmarkWeb, :controller
  alias Benchmark.{ApiToken, Post, Repo}
  import Ecto.Query

  def index(conn, _params) do
    posts = Post.recent(Repo)
    json(conn, posts)
  end

  def show(conn, %{"id" => id}) do
    case Repo.get(Post, id) do
      nil  -> conn |> put_status(404) |> json(%{error: "not found"})
      post -> json(conn, post)
    end
  end

  def create(conn, params) do
    changeset = Post.changeset(%Post{}, params)
    case Repo.insert(changeset) do
      {:ok, post}             -> conn |> put_status(201) |> json(post)
      {:error, changeset} ->
        errors = Ecto.Changeset.traverse_errors(changeset, fn {msg, _opts} -> msg end)
        conn |> put_status(422) |> json(%{error: inspect(errors)})
    end
  end

  def update(conn, %{"id" => id} = params) do
    case Repo.get(Post, id) do
      nil  -> conn |> put_status(404) |> json(%{error: "not found"})
      post ->
        changeset = Post.changeset(post, params)
        case Repo.update(changeset) do
          {:ok, updated}      -> json(conn, updated)
          {:error, changeset} ->
            errors = Ecto.Changeset.traverse_errors(changeset, fn {msg, _opts} -> msg end)
            conn |> put_status(422) |> json(%{error: inspect(errors)})
        end
    end
  end

  def delete(conn, %{"id" => id}) do
    case Repo.get(Post, id) do
      nil  -> conn |> put_status(404) |> json(%{error: "not found"})
      post ->
        Repo.delete!(post)
        send_resp(conn, 204, "")
    end
  end

  def protected(conn, _params) do
    case get_req_header(conn, "authorization") do
      ["Bearer " <> token | _] ->
        case ApiToken.verify(Repo, token) do
          {:ok, principal} ->
            total = Repo.one(from p in Post, select: count(p.id))
            json(conn, %{principal: principal, total_posts: total})
          {:error, _} ->
            conn |> put_status(401) |> json(%{error: "invalid token"})
        end
      _ ->
        conn |> put_status(401) |> json(%{error: "missing or invalid Authorization header"})
    end
  end
end
