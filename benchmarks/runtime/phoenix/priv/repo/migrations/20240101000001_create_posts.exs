defmodule Benchmark.Repo.Migrations.CreatePosts do
  use Ecto.Migration

  def change do
    create table(:posts) do
      add :title,     :text,    null: false
      add :body,      :text,    null: false
      add :published, :boolean, null: false, default: false
      add :author,    :text,    null: false
      timestamps(inserted_at: :created_at, type: :utc_datetime)
    end

    create table(:api_tokens) do
      add :token,     :text, null: false
      add :principal, :text, null: false
      timestamps(inserted_at: :created_at, updated_at: false, type: :utc_datetime)
    end

    create unique_index(:api_tokens, [:token])
  end
end
