require_relative "boot"

require "rails"
require "active_model/railtie"
require "active_record/railtie"
require "action_controller/railtie"
require "action_view/railtie"

Bundler.require(*Rails.groups)

module BenchRails
  class Application < Rails::Application
    config.load_defaults 7.2

    config.eager_load = true
    config.enable_reloading = false
    config.hosts.clear
    config.log_level = :warn
    config.public_file_server.enabled = ENV["RAILS_SERVE_STATIC_FILES"].present?
    config.secret_key_base = ENV.fetch("SECRET_KEY_BASE")
  end
end
