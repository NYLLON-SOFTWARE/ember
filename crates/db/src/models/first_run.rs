//! `reference/app/models/first_run.rb`

use crate::database::Tx;
use crate::error::Result;
use crate::models::{Account, NewUser, PasswordDigest, Role, Room, RoomType, User};

pub struct FirstRun;

impl FirstRun {
    pub const ACCOUNT_NAME: &'static str = "Matchbox";
    pub const FIRST_ROOM_NAME: &'static str = "All Talk";

    /// `FirstRun.create!(user_params)`: the account, an administrator, and the first (open)
    /// room, which the administrator joins. Rails runs these as separate saves; the room and
    /// user callbacks (open room grants, user's open-room memberships) run after commit and
    /// the explicit grant follows them.
    pub fn create(tx: &mut Tx<'_>, name: &str, email_address: &str, password_digest: PasswordDigest) -> Result<User> {
        Account::create(tx, Self::ACCOUNT_NAME)?;
        let administrator = User::create(
            tx,
            NewUser {
                name: name.into(),
                email_address: Some(email_address.into()),
                password_digest: Some(password_digest),
                role: Role::Administrator,
                ..Default::default()
            },
        )?;
        let room = Room::create(tx, RoomType::Open, Some(Self::FIRST_ROOM_NAME), administrator.id)?;
        let administrator_id = administrator.id;
        tx.after_commit(move |tx| room.grant_to(tx, &[administrator_id]));
        Ok(administrator)
    }
}
