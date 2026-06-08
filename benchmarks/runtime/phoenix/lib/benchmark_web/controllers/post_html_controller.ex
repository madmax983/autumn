defmodule BenchmarkWeb.PostHtmlController do
  use BenchmarkWeb, :controller
  alias Benchmark.{Post, Repo}

  def index(conn, _params) do
    posts = Post.recent(Repo)
    rows =
      Enum.map_join(posts, "", fn post ->
        draft = if post.published, do: "", else: " <em>[draft]</em>"

        """
        <li><a href="/posts/#{post.id}">#{esc(post.title)}</a> &mdash; #{esc(post.author)}#{draft}</li>
        """
      end)

    html(conn, """
    <!DOCTYPE html>
    <html lang="en">
    <head><meta charset="utf-8"><title>Posts</title></head>
    <body><h1>Posts</h1><ul>#{rows}</ul></body>
    </html>
    """)
  end

  def show(conn, %{"id" => id}) do
    case Repo.get(Post, id) do
      nil  -> conn |> put_status(404) |> text("not found")
      post ->
        draft = if post.published, do: "", else: "<em>Draft</em>"

        html(conn, """
        <!DOCTYPE html>
        <html lang="en">
        <head><meta charset="utf-8"><title>#{esc(post.title)}</title></head>
        <body>
          <h1>#{esc(post.title)}</h1>
          <p>By #{esc(post.author)}</p>
          #{draft}
          <div>#{esc(post.body)}</div>
        </body>
        </html>
        """)
    end
  end

  defp esc(value) do
    value
    |> to_string()
    |> Phoenix.HTML.html_escape()
    |> Phoenix.HTML.safe_to_string()
  end
end
