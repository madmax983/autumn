namespace :bench do
  desc "Seed benchmark database with 1000 posts and 1 API token"
  task seed: :environment do
    Post.delete_all
    ApiToken.delete_all
    ActiveRecord::Base.connection.reset_pk_sequence!(:posts)
    ActiveRecord::Base.connection.reset_pk_sequence!(:api_tokens)

    authors = %w[alice bob carol dave eve]
    body_suffix = "Lorem ipsum dolor sit amet. " * 3

    posts = (1..1000).map do |n|
      {
        title:     "Post number #{n}",
        body:      "This is the body of post number #{n}. It contains enough text to be realistic. #{body_suffix}",
        published: (n % 3 != 0),
        author:    authors[n % 5],
        created_at: Time.now,
        updated_at: Time.now,
      }
    end
    Post.insert_all(posts)
    ApiToken.create!(token: "benchmark-token-abc123", principal: "benchmark-user")
    puts "Seeded 1000 posts and 1 API token."
  end
end
