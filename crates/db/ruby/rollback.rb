# Boots the reference app on a database ember_db wrote (`export_database_for_rails` in
# src/tests/fixtures_test.rs) and reads, edits, searches and deletes through Active Record, the
# way Rails would after a rollback. Run by reference-tools/db/differential.sh.
ActiveJob::Base.queue_adapter = :test
def check(what) = (yield or raise "rollback: #{what}")

message = Message.find_by!(client_message_id: "rust-1")
check("message body") { message.plain_text_body == "Written by Rust hovercraft" }
check("message room and creator") { message.room.name == "Designers" && message.creator.name == "David" }
check("boost") { message.boosts.sole.then { _1.content == "🦀" && _1.booster.name == "Jason" } }
check("search finds the Rust message") { Message.search("hovercraft").include?(message) }

rusty = User.find_by!(email_address: "rusty@example.com")
check("password") { rusty.authenticate("secret123456") }
check("session") { rusty.sessions.sole.ip_address == "8.8.8.8" }
check("search record") { rusty.searches.pluck(:query) == [ "hovercraft" ] }
room = Room.find_by!(name: "Rust Room")
check("closed room") { room.is_a?(Rooms::Closed) && room.users.pluck(:name).sort == %w[ David Rusty ] }
check("account settings") { Account.first.settings.restrict_room_creation_to_administrators == true }

Current.user = rusty
message.update!(body: "Edited by Rails zeppelin")
check("edit is searchable") { Message.search("zeppelin").include?(message) && Message.search("hovercraft").exclude?(message) }
message.destroy!
check("delete") { Message.search("zeppelin").none? && Boost.where(message_id: message.id).none? }
puts "rollback ok"
