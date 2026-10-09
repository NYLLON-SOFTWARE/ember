//! `ember_db::RichText` over `ember_richtext`, for the models' Action Text needs
//! (`plain_text_body` for FTS, push and webhooks; `mentionees`).
//!
//! The models call this on the database writer thread inside their transaction, or on a reader
//! they hold. Record lookups use that same connection with the one resolver implementation the
//! controllers use (`controllers::presenters::DbResolver`): checking out another connection here
//! deadlocks once every pooled reader is waiting on the writer.

use std::sync::Arc;

use ember_db::{Connection, RichText};
use ember_kit::SharedClock;
use ember_richtext::RenderContext;
use rails_compat::Secrets;

use crate::controllers::presenters::DbResolver;

pub struct AppRichText {
    secrets: Arc<Secrets>,
    clock: SharedClock,
}

impl AppRichText {
    pub fn new(secrets: Arc<Secrets>, clock: SharedClock) -> Self {
        Self { secrets, clock }
    }

    fn with_context<T>(&self, conn: &Connection, f: impl FnOnce(&RenderContext) -> T) -> T {
        let resolver = DbResolver { conn, secrets: &self.secrets, now: self.clock.now() };
        f(&resolver.render_context(None))
    }
}

impl RichText for AppRichText {
    /// `message.body.to_plain_text`. Where it raises, Rails fails the save after its commit; here
    /// it's logged and the body has no plain text (see "Known differences" in the README).
    fn to_plain_text(&self, conn: &Connection, html: &str) -> String {
        match self.with_context(conn, |ctx| ember_richtext::to_plain_text(html, ctx)) {
            Ok(text) => text,
            Err(error) => {
                tracing::error!(%error, "to_plain_text raised");
                String::new()
            }
        }
    }

    /// `body.attachables.grep(User).uniq`: verified SGIDs only.
    fn mentioned_user_ids(&self, conn: &Connection, html: &str) -> Vec<i64> {
        match self.with_context(conn, |ctx| ember_richtext::mentioned_users(html, ctx)) {
            Ok(users) => users.into_iter().map(|user| user.id).collect(),
            Err(error) => {
                tracing::error!(%error, "mentioned_users raised");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_whose_plain_text_raises_has_none() {
        let rich_text = AppRichText::new(Arc::new(Secrets::new("test-secret")), Arc::new(ember_kit::SystemClock));
        let conn = Connection::open_in_memory().unwrap();
        // Rails raises `ArgumentError: invalid base64` reading the mention.
        let html =
            r#"Hey <action-text-attachment sgid="!!!" content-type="application/vnd.campfire.mention"></action-text-attachment> there"#;
        assert_eq!(rich_text.to_plain_text(&conn, html), "");
        assert_eq!(rich_text.to_plain_text(&conn, "Hey <b>there</b>"), "Hey there");
    }
}
