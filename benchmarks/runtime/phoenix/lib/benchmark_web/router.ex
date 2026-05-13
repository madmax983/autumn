defmodule BenchmarkWeb.Router do
  use BenchmarkWeb, :router

  pipeline :browser do
    plug :accepts, ["html"]
    plug :put_root_layout, html: {BenchmarkWeb.Layouts, :root}
  end

  pipeline :api do
    plug :accepts, ["json"]
  end

  scope "/", BenchmarkWeb do
    pipe_through :browser
    get "/posts",      PostHtmlController, :index
    get "/posts/:id",  PostHtmlController, :show
    get "/health",     HealthController, :index
  end

  scope "/api", BenchmarkWeb do
    pipe_through :api
    get    "/posts/protected", PostApiController, :protected
    get    "/posts",           PostApiController, :index
    get    "/posts/:id",       PostApiController, :show
    post   "/posts",           PostApiController, :create
    patch  "/posts/:id",       PostApiController, :update
    delete "/posts/:id",       PostApiController, :delete
  end
end
