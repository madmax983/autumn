defmodule Benchmark.Post do
  use Ecto.Schema
  import Ecto.Changeset
  import Ecto.Query

  @derive {Jason.Encoder, only: [:id, :title, :body, :published, :author, :created_at, :updated_at]}

  schema "posts" do
    field :title,     :string
    field :body,      :string
    field :published, :boolean, default: false
    field :author,    :string
    timestamps(inserted_at: :created_at, type: :utc_datetime)
  end

  def changeset(post, attrs) do
    post
    |> cast(attrs, [:title, :body, :published, :author])
    |> validate_required([:title, :body, :author])
    |> validate_length(:title, max: 255, message: "must be 255 characters or fewer")
    |> validate_length(:title, min: 1, message: "must not be blank")
  end

  def recent(repo) do
    repo.all(from p in __MODULE__, order_by: [desc: p.created_at], limit: 50)
  end
end
