class ApiToken < ApplicationRecord
  def self.verify(raw_token)
    find_by(token: raw_token)&.principal
  end
end
