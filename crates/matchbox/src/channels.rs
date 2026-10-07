//! Action Cable: `reference/app/channels`, the broadcasts the app makes (`Broadcasts`), and
//! revocation (`revocation`). `app.rs` builds the cable server with these channels and routes the
//! models' events to `Broadcasts` and `revocation`.
//!
//! Every channel matches its Ruby class: identifier, streams, payloads, and callback order
//! (`on_subscribe` runs after `subscribed`, before the confirmation). Ruby makes every public
//! method an action, including a publicly redefined `subscribed`, so those are performable here
//! too.

pub mod broadcasts;
mod connection;
mod presence;
mod read_rooms;
pub mod revocation;
mod room;
mod room_messages;
mod typing_notifications;
mod unread_rooms;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use matchbox_cable::turbo::{STREAMS_CHANNEL, StreamsChannel};
use matchbox_cable::{Config, EmptyChannel, Identified, Server, ServerBuilder};
use matchbox_db::Database;
use rails_compat::Secrets;
use rails_compat::global_id::GlobalId;

pub use broadcasts::{Broadcasts, Partials};
pub use connection::SessionAuthenticator;

/// The cable server, identified by `current_user`.
pub type Cable = Server<CableUser>;

/// `identified_by :current_user`: the user as loaded when the connection opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CableUser {
    pub id: i64,
    pub name: String,
}

impl Identified for CableUser {
    /// `connection_gid`: the user's GlobalID, which `remote_connections.where(current_user:)`
    /// matches.
    fn connection_identifier(&self) -> String {
        user_gid(self.id).to_string()
    }
}

/// What the cable server needs from the app.
#[derive(Clone)]
pub struct Deps {
    pub db: Database,
    pub secrets: Arc<Secrets>,
    pub clock: matchbox_kit::SharedClock,
}

/// The cable server with `ApplicationCable::Connection` and every channel registered. Mount it
/// with `cable.router("/cable")`.
pub fn server(deps: Deps, config: Config) -> Cable {
    let authenticator = SessionAuthenticator::new(deps.db.clone(), deps.secrets.clone(), deps.clock.clone());
    register(Server::builder(config, authenticator), &deps.db, StreamsChannel::new(deps.secrets.clone())).build()
}

/// Registers the channels under their Ruby class names. `streams` verifies signed stream names
/// (`Turbo.signed_stream_verifier`); `RoomMessagesChannel` verifies with it too, and
/// `Turbo::StreamsChannel` gets it with `RoomStreamsAreAuthorized` prepended.
pub fn register(builder: ServerBuilder<CableUser>, db: &Database, streams: StreamsChannel) -> ServerBuilder<CableUser> {
    let (presence_db, room_db, messages_db, typing_db) = (db.clone(), db.clone(), db.clone(), db.clone());
    let stock = streams.clone().guarded_by(room_messages::guarded_stream);
    builder
        .channel("ApplicationCable::Channel", || EmptyChannel)
        .channel("HeartbeatChannel", || EmptyChannel)
        .channel("PresenceChannel", move || presence::PresenceChannel::new(presence_db.clone()))
        .channel("ReadRoomsChannel", || read_rooms::ReadRoomsChannel)
        .channel("RoomChannel", move || room::RoomChannel::new(room_db.clone()))
        .channel("RoomMessagesChannel", move || room_messages::RoomMessagesChannel::new(messages_db.clone(), streams.clone()))
        .channel("TypingNotificationsChannel", move || typing_notifications::TypingNotificationsChannel::new(typing_db.clone()))
        .channel("UnreadRoomsChannel", || unread_rooms::UnreadRoomsChannel)
        .channel(STREAMS_CHANNEL, move || stock.clone())
}

pub fn user_gid(user_id: i64) -> GlobalId {
    GlobalId::new("User", user_id)
}

/// A room's GlobalID names its STI class (`gid://campfire/Rooms::Open/1`).
pub fn room_gid(room: &matchbox_db::Room) -> GlobalId {
    GlobalId::new(room.room_type.class_name(), room.id)
}
