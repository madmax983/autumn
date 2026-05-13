class Post < ApplicationRecord
  validates :title, presence: true, length: { maximum: 255 }
  validates :body,  presence: true
  validates :author, presence: true

  scope :recent, -> { order(created_at: :desc).limit(50) }
end
