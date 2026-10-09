//! Action Text's record lookups for `ember_richtext`, backed by the database: mention
//! attachments resolve to users through their signed (or, for users only, unverified) GlobalIDs
//! (`reference/lib/rails_ext/action_text_attachables.rb`).

use ember_db::{Connection, User};
use ember_richtext::{AttachableResolver, GidLookup, MentionUser, RenderContext, SignedLookup};
use rails_compat::Secrets;
use rails_compat::global_id::{self, ATTACHABLE_PURPOSE, GlobalId};

use super::avatar_path;

pub struct DbResolver<'a> {
    pub conn: &'a Connection,
    pub secrets: &'a Secrets,
    pub now: jiff::Timestamp,
}

impl DbResolver<'_> {
    fn mention_user(&self, user: &User) -> MentionUser {
        MentionUser {
            id: user.id,
            name: user.name.clone(),
            title: user.title(),
            attachable_sgid: global_id::attachable_sgid(self.secrets, &GlobalId::new("User", user.id)),
            user_path: ember_routes::user(user.id),
            avatar_path: avatar_path(self.secrets, user),
        }
    }

    /// `GlobalID::Locator` finds records by the model's primary key; ids that aren't integers
    /// never match a row.
    fn find_user(&self, id: &str) -> Option<User> {
        let id: i64 = id.parse().ok()?;
        User::find_by_id(self.conn, id).ok().flatten()
    }

    pub fn render_context(&self, request_host: Option<String>) -> RenderContext<'_> {
        RenderContext { resolver: self, request_host }
    }
}

impl AttachableResolver for DbResolver<'_> {
    fn locate_signed(&self, sgid: &str) -> SignedLookup {
        let Some(gid) = global_id::locate_signed(self.secrets, sgid, ATTACHABLE_PURPOSE, self.now) else {
            return SignedLookup::Invalid;
        };
        match gid.model_name.as_str() {
            "User" => match self.find_user(&gid.id) {
                Some(user) => SignedLookup::User(self.mention_user(&user)),
                None => SignedLookup::MissingRecord { model_name: gid.model_name },
            },
            _ => SignedLookup::MissingRecord { model_name: gid.model_name },
        }
    }

    fn find_gid(&self, gid: &str) -> GidLookup {
        let Some(gid) = GlobalId::parse(gid) else { return GidLookup::NotFound };
        match gid.model_name.as_str() {
            "User" => match self.find_user(&gid.id) {
                Some(user) => GidLookup::User(self.mention_user(&user)),
                None => GidLookup::NotFound,
            },
            "Message" | "Room" | "Rooms::Open" | "Rooms::Closed" | "Rooms::Direct" | "Boost" | "Account" => GidLookup::OtherModel,
            _ => GidLookup::Raises,
        }
    }
}
