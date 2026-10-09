//! What the models need from Action Text. The real pipeline lives in `ember_richtext`;
//! the app plugs it in through [`RichText`]. Tests have a stand-in, `testing::BasicRichText`.

use rusqlite::Connection;

/// Both methods get the connection the caller is already on (the writer's transaction, or the
/// reader it holds) for any record lookups: they must never check out another connection, which
/// deadlocks once every pooled reader waits on the writer.
pub trait RichText: Send + Sync {
    /// `ActionText::Content#to_plain_text` of a stored body. Mention attachments render as
    /// `attachable_plain_text_representation`, i.e. `"@#{name}"`
    /// (`reference/app/models/user/mentionable.rb`).
    fn to_plain_text(&self, conn: &Connection, html: &str) -> String;

    /// `body.attachables.grep(User).uniq`: user ids from mention attachments, in document
    /// order, deduplicated (`reference/app/models/message/mentionee.rb`).
    fn mentioned_user_ids(&self, conn: &Connection, html: &str) -> Vec<i64>;
}
