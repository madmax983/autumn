Rails.application.routes.draw do
  resources :posts, only: [:index, :show]

  namespace :api do
    resources :posts, only: [:index, :show, :create, :update, :destroy] do
      collection do
        get :protected
      end
    end
  end

  get "/health", to: proc { [200, {}, ["ok"]] }
end
