//! `test/models/account_test.rb`, `test/models/account/joinable_test.rb`

use super::*;
use crate::Account;

fn signal(t: &TestDb) -> Account {
    t.read(|c| Ok(Account::first(c)?.unwrap()))
}

#[test]
fn branding_changes_the_default_display_name_without_overwriting_stored_names() {
    let t = TestDb::new();
    let mut account = signal(&t);
    account.name = "Campfire".into();
    assert_eq!(account.display_name(), "Matchbox");
    assert_eq!(account.name, "Campfire");
    account.name = "Our team".into();
    assert_eq!(account.display_name(), "Our team");
}

#[test]
fn settings() {
    let t = TestDb::new();
    let mut account = signal(&t);
    assert_eq!(account.settings_json, None, "fixture leaves settings NULL");

    let mut settings = account.settings();
    settings.set_restrict_room_creation_to_administrators("true");
    assert!(settings.restrict_room_creation_to_administrators());
    assert_eq!(settings.to_json(), r#"{"restrict_room_creation_to_administrators":true}"#);

    let mut a = account.clone();
    t.write(move |tx| a.update(tx, None, None, Some(&[("restrict_room_creation_to_administrators", "true")])));
    account.reload_from(&t);
    assert!(account.settings().restrict_room_creation_to_administrators());
    assert_eq!(account.settings_json.as_deref(), Some(r#"{"restrict_room_creation_to_administrators":true}"#));

    let mut settings = account.settings();
    settings.set_restrict_room_creation_to_administrators("false");
    assert!(!settings.restrict_room_creation_to_administrators());
    assert_eq!(settings.to_json(), r#"{"restrict_room_creation_to_administrators":false}"#);

    let mut a = account.clone();
    t.write(move |tx| a.update(tx, None, None, Some(&[("restrict_room_creation_to_administrators", "false")])));
    account.reload_from(&t);
    assert!(!account.settings().restrict_room_creation_to_administrators());
}

#[test]
fn translation_visibility_defaults_to_hidden_and_persists_without_changing_other_settings() {
    let t = TestDb::new();
    let mut account = signal(&t);
    assert!(account.settings().hide_translation_buttons());
    assert_eq!(account.settings_json, None);
    let mut copy = account.clone();
    t.write(move |tx| copy.update(tx, None, None, Some(&[("hide_translation_buttons", "false")])));
    account.reload_from(&t);
    assert!(!account.settings().hide_translation_buttons());
    let mut copy = account.clone();
    t.write(move |tx| copy.update(tx, None, None, Some(&[("restrict_room_creation_to_administrators", "true")])));
    account.reload_from(&t);
    assert!(!account.settings().hide_translation_buttons());
    assert!(account.settings().restrict_room_creation_to_administrators());
    let mut copy = account.clone();
    t.write(move |tx| copy.update(tx, None, None, Some(&[("hide_translation_buttons", "true")])));
    account.reload_from(&t);
    assert!(account.settings().hide_translation_buttons());
    assert!(account.settings().restrict_room_creation_to_administrators());
}

#[test]
fn updating_other_attributes_leaves_null_settings_alone() {
    // What Rails does: `update!(name:)` on the fixture account doesn't write settings.
    let t = TestDb::new();
    let mut account = signal(&t);
    t.write(move |tx| account.update(tx, Some("X"), None, None));
    let account = signal(&t);
    assert_eq!(account.name, "X");
    assert_eq!(account.settings_json, None);
}

#[test]
fn unknown_settings_are_rejected() {
    let t = TestDb::new();
    let mut account = signal(&t);
    assert!(t.try_write(move |tx| account.update(tx, None, None, Some(&[("nope", "1")]))).is_err());
}

#[test]
fn new_accounts_get_a_joinable_code() {
    let t = TestDb::new();
    let account = t.write(|tx| {
        tx.conn().execute("DELETE FROM accounts", [])?;
        Account::create(tx, "Chat")
    });
    let parts: Vec<&str> = account.join_code.split('-').collect();
    assert_eq!(parts.len(), 3);
    assert!(parts.iter().all(|p| p.len() == 4 && p.chars().all(|c| c.is_ascii_alphanumeric())));
    assert_eq!(account.settings_json.as_deref(), Some(r#"{"restrict_room_creation_to_administrators":false}"#));
    assert_eq!(account.singleton_guard, 0);
}

#[test]
fn only_one_account_can_exist() {
    let t = TestDb::new();
    assert!(t.try_write(|tx| Account::create(tx, "Second")).is_err());
}

#[test]
fn accounts_can_reset_join_code() {
    let t = TestDb::new();
    let before = signal(&t).join_code;
    let mut account = signal(&t);
    t.write(move |tx| account.reset_join_code(tx));
    assert_ne!(signal(&t).join_code, before);
}

#[test]
fn channel_orders_are_personal_and_reset_without_touching_account_caches() {
    let t = TestDb::new();
    let before = signal(&t);
    assert!(before.settings().channel_order(id("david")).is_empty());
    assert!(t.write(|tx| Account::set_channel_order(tx, id("david"), &[id("hq"), id("pets")])));
    assert!(t.write(|tx| Account::set_channel_order(tx, id("jason"), &[id("pets"), id("hq")])));

    let settings = signal(&t).settings();
    assert_eq!(settings.channel_order(id("david")), vec![id("hq"), id("pets")]);
    assert_eq!(settings.channel_order(id("jason")), vec![id("pets"), id("hq")]);
    assert!(settings.channel_order(id("kevin")).is_empty());
    assert_eq!(signal(&t).updated_at, before.updated_at, "personal ordering does not invalidate account/asset caches");

    assert!(t.write(|tx| Account::set_channel_order(tx, id("david"), &[])));
    assert!(signal(&t).settings().channel_order(id("david")).is_empty());
    assert_eq!(signal(&t).settings().channel_order(id("jason")), vec![id("pets"), id("hq")]);
}

#[test]
fn channel_orders_reject_duplicates_direct_rooms_and_inaccessible_channels() {
    let t = TestDb::new();
    assert!(t.write(|tx| Account::set_channel_order(tx, id("kevin"), &[id("hq")])));
    let before = signal(&t).settings_json;
    for ids in [vec![id("pets")], vec![id("david_and_kevin")], vec![id("hq"), id("hq")], vec![0], vec![-1]] {
        assert!(!t.write(move |tx| Account::set_channel_order(tx, id("kevin"), &ids)));
        assert_eq!(signal(&t).settings_json, before);
    }
    t.write(|tx| {
        let mut membership = crate::Membership::find_by_room_and_user(tx.conn(), id("hq"), id("kevin"))?.unwrap();
        membership.update_involvement(tx, crate::Involvement::Invisible)
    });
    assert!(!t.write(|tx| Account::set_channel_order(tx, id("kevin"), &[id("hq")])));
    assert_eq!(signal(&t).settings_json, before);
}

#[test]
fn personal_channel_order_survives_an_admin_update_from_an_older_account_snapshot() {
    let t = TestDb::new();
    let mut stale_account = signal(&t);
    assert!(t.write(|tx| Account::set_channel_order(tx, id("david"), &[id("hq"), id("pets")])));
    t.write(move |tx| stale_account.update(tx, None, None, Some(&[("hide_translation_buttons", "false")])));
    let settings = signal(&t).settings();
    assert!(!settings.hide_translation_buttons());
    assert_eq!(settings.channel_order(id("david")), vec![id("hq"), id("pets")]);

    assert!(t.write(|tx| Account::set_channel_order(tx, id("david"), &[])));
    assert!(!signal(&t).settings().hide_translation_buttons(), "reset does not affect workspace settings");
}

#[test]
fn channel_favorites_are_personal_and_preserve_other_settings_and_caches() {
    let t = TestDb::new();
    let mut stale_account = signal(&t);
    let updated_at = stale_account.updated_at;
    assert!(t.write(|tx| Account::set_channel_order(tx, id("david"), &[id("pets"), id("hq")])));
    assert_eq!(t.write(|tx| Account::set_channel_favorite(tx, id("david"), id("hq"), true)), Some(vec![id("hq")]));
    assert_eq!(t.write(|tx| Account::set_channel_favorite(tx, id("david"), id("hq"), true)), Some(vec![id("hq")]));
    assert_eq!(t.write(|tx| Account::set_channel_favorite(tx, id("jason"), id("pets"), true)), Some(vec![id("pets")]));
    assert_eq!(signal(&t).updated_at, updated_at);
    t.write(move |tx| stale_account.update(tx, None, None, Some(&[("hide_translation_buttons", "false")])));
    let settings = signal(&t).settings();
    assert_eq!(settings.favorite_channels(id("david")), vec![id("hq")]);
    assert_eq!(settings.favorite_channels(id("jason")), vec![id("pets")]);
    assert!(settings.favorite_channels(id("kevin")).is_empty());
    assert_eq!(settings.channel_order(id("david")), vec![id("pets"), id("hq")]);
    assert!(!settings.hide_translation_buttons());
    assert_eq!(t.write(|tx| Account::set_channel_favorite(tx, id("david"), id("hq"), false)), Some(vec![]));
    assert_eq!(signal(&t).settings().favorite_channels(id("jason")), vec![id("pets")]);
}

#[test]
fn channel_favorites_reject_inaccessible_hidden_and_direct_rooms() {
    let t = TestDb::new();
    let before = signal(&t).settings_json;
    for room_id in [id("pets"), id("david_and_kevin"), 0, -1] {
        assert_eq!(t.write(move |tx| Account::set_channel_favorite(tx, id("kevin"), room_id, true)), None);
        assert_eq!(signal(&t).settings_json, before);
    }
    t.write(|tx| {
        let mut membership = crate::Membership::find_by_room_and_user(tx.conn(), id("hq"), id("kevin"))?.unwrap();
        membership.update_involvement(tx, crate::Involvement::Invisible)
    });
    assert_eq!(t.write(|tx| Account::set_channel_favorite(tx, id("kevin"), id("hq"), true)), None);
    assert_eq!(signal(&t).settings_json, before);
}

impl Account {
    fn reload_from(&mut self, t: &TestDb) {
        let id = self.id;
        *self = t.read(|c| Account::find(c, id));
    }
}

#[test]
fn default_room_order_requires_an_admin_and_preserves_preferences_and_caches() {
    let t = TestDb::new();
    let mut stale_account = signal(&t);
    let updated_at = stale_account.updated_at;
    assert!(!t.write(|tx| Account::set_default_room_order(tx, id("kevin"), &[id("hq")])));
    assert!(t.write(|tx| Account::set_channel_order(tx, id("kevin"), &[id("hq")])));
    assert!(t.write(|tx| Account::set_default_room_order(tx, id("david"), &[id("pets"), id("hq")])));
    assert_eq!(signal(&t).updated_at, updated_at);
    t.write(move |tx| stale_account.update(tx, None, None, Some(&[("hide_translation_buttons", "false")])));
    let settings = signal(&t).settings();
    assert_eq!(settings.default_room_order(), vec![id("pets"), id("hq")]);
    assert_eq!(settings.channel_order(id("kevin")), vec![id("hq")]);
    assert!(!settings.hide_translation_buttons());
    for ids in [vec![id("hq"), id("hq")], vec![id("david_and_kevin")], vec![0], vec![i64::MAX]] {
        assert!(!t.write(move |tx| Account::set_default_room_order(tx, id("david"), &ids)));
        assert_eq!(signal(&t).settings(), settings);
    }
    assert!(t.write(|tx| Account::set_default_room_order(tx, id("david"), &[])));
    assert!(signal(&t).settings().default_room_order().is_empty());
    assert_eq!(signal(&t).settings().channel_order(id("kevin")), vec![id("hq")]);
}

#[test]
fn default_room_order_keeps_slots_for_rooms_the_updating_admin_cannot_see() {
    let t = TestDb::new();
    assert!(t.write(|tx| Account::set_default_room_order(tx, id("david"), &[id("pets"), id("hq")])));
    t.write(|tx| {
        let mut membership = crate::Membership::find_by_room_and_user(tx.conn(), id("pets"), id("jason"))?.unwrap();
        membership.update_involvement(tx, crate::Involvement::Invisible)
    });
    assert!(!t.write(|tx| Account::set_default_room_order(tx, id("jason"), &[id("pets")])));
    assert!(t.write(|tx| Account::set_default_room_order(tx, id("jason"), &[id("hq")])));
    assert_eq!(signal(&t).settings().default_room_order(), vec![id("pets"), id("hq")]);
}
