class CreatePosts < ActiveRecord::Migration[7.2]
  def change
    create_table :posts do |t|
      t.text    :title,     null: false
      t.text    :body,      null: false
      t.boolean :published, null: false, default: false
      t.text    :author,    null: false
      t.timestamps null: false
    end

    create_table :api_tokens do |t|
      t.text      :token,     null: false
      t.text      :principal, null: false
      t.timestamp :created_at, null: false, default: -> { "NOW()" }
    end
    add_index :api_tokens, :token, unique: true
  end
end
