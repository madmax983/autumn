defmodule Benchmark.ApiToken do
  use Ecto.Schema
  import Ecto.Query

  schema "api_tokens" do
    field :token,     :string
    field :principal, :string
    timestamps(inserted_at: :created_at, updated_at: false)
  end

  def verify(repo, raw_token) do
    case repo.one(from t in __MODULE__, where: t.token == ^raw_token, select: t.principal) do
      nil       -> {:error, :invalid}
      principal -> {:ok, principal}
    end
  end
end
