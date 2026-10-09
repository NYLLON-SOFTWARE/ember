//! `ApplicationCable::Connection` (reference/app/channels/application_cable/connection.rb):
//! `current_user` from the signed `session_token` cookie (`Authentication::SessionLookup`), or
//! `reject_unauthorized_connection`.
use std::sync::Arc;

use ember_cable::{Authenticate, ConnectRequest};
use ember_db::{Database, Session, User};
use ember_kit::{CookieJar, SharedClock};
use rails_compat::Secrets;

use super::CableUser;

pub struct SessionAuthenticator {
    db: Database,
    secrets: Arc<Secrets>,
    clock: SharedClock,
}

impl SessionAuthenticator {
    pub fn new(db: Database, secrets: Arc<Secrets>, clock: SharedClock) -> Self {
        Self { db, secrets, clock }
    }

    /// `cookies.signed[:session_token]`.
    fn session_token(&self, request: &ConnectRequest) -> Option<String> {
        let headers = request.headers.get_all("cookie").iter().filter_map(|value| value.to_str().ok());
        CookieJar::from_headers(headers, self.secrets.clone(), self.clock.clone()).signed("session_token")
    }
}

#[async_trait::async_trait]
impl Authenticate<CableUser> for SessionAuthenticator {
    async fn connect(&self, request: &ConnectRequest) -> Option<CableUser> {
        let token = self.session_token(request)?;
        let user = self
            .db
            .read(move |conn| match Session::find_by_token(conn, &token)? {
                Some(session) => User::find_by_id(conn, session.user_id),
                None => Ok(None),
            })
            .await;
        match user {
            Ok(user) => user.map(|user| CableUser { id: user.id, name: user.name }),
            Err(error) => {
                tracing::error!(%error, "Could not look up the cable session");
                None
            }
        }
    }
}
