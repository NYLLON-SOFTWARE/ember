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
    assert_eq!(account.display_name(), "Ember");
    assert_eq!(account.name, "Campfire");
    account.name = "Matchbox".into();
    assert_eq!(account.display_name(), "Ember");
    assert_eq!(account.name, "Matchbox");
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
fn favorite_orders_are_personal_and_preserve_account_caches() {
    let t = TestDb::new();
    let before = signal(&t);
    for user in [id("david"), id("jason")] {
        for room in [id("hq"), id("pets")] {
            assert!(t.write(move |tx| Account::set_channel_favorite(tx, user, room, true)).is_some());
        }
    }
    assert!(t.write(|tx| Account::set_favorite_order(tx, id("david"), &[id("pets"), id("hq")])));
    let settings = signal(&t).settings();
    assert_eq!(settings.favorite_channels(id("david")), vec![id("pets"), id("hq")]);
    assert_eq!(settings.favorite_channels(id("jason")), vec![id("hq"), id("pets")]);
    assert!(settings.favorite_channels(id("kevin")).is_empty());
    assert_eq!(signal(&t).updated_at, before.updated_at);
    assert_eq!(t.write(|tx| Account::set_channel_favorite(tx, id("david"), id("pets"), true)), Some(vec![id("pets"), id("hq")]));
}

#[test]
fn favorite_orders_reject_unstarred_missing_duplicate_hidden_and_direct_rooms() {
    let t = TestDb::new();
    assert!(t.write(|tx| Account::set_channel_favorite(tx, id("david"), id("hq"), true)).is_some());
    let before = signal(&t).settings_json;
    for ids in [vec![], vec![id("pets")], vec![id("hq"), id("pets")], vec![id("david_and_kevin")], vec![id("hq"), id("hq")], vec![0]] {
        assert!(!t.write(move |tx| Account::set_favorite_order(tx, id("david"), &ids)));
        assert_eq!(signal(&t).settings_json, before);
    }
    t.write(|tx| {
        let mut membership = crate::Membership::find_by_room_and_user(tx.conn(), id("hq"), id("david"))?.unwrap();
        membership.update_involvement(tx, crate::Involvement::Invisible)
    });
    assert!(!t.write(|tx| Account::set_favorite_order(tx, id("david"), &[id("hq")])));
    assert_eq!(signal(&t).settings_json, before);
}

#[test]
fn channel_favorites_are_personal_and_preserve_other_settings_and_caches() {
    let t = TestDb::new();
    let mut stale_account = signal(&t);
    let updated_at = stale_account.updated_at;
    assert_eq!(t.write(|tx| Account::set_channel_favorite(tx, id("david"), id("hq"), true)), Some(vec![id("hq")]));
    assert_eq!(t.write(|tx| Account::set_channel_favorite(tx, id("david"), id("hq"), true)), Some(vec![id("hq")]));
    assert_eq!(t.write(|tx| Account::set_channel_favorite(tx, id("jason"), id("pets"), true)), Some(vec![id("pets")]));
    assert_eq!(signal(&t).updated_at, updated_at);
    t.write(move |tx| stale_account.update(tx, None, None, Some(&[("hide_translation_buttons", "false")])));
    let settings = signal(&t).settings();
    assert_eq!(settings.favorite_channels(id("david")), vec![id("hq")]);
    assert_eq!(settings.favorite_channels(id("jason")), vec![id("pets")]);
    assert!(settings.favorite_channels(id("kevin")).is_empty());
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
fn default_room_order_replaces_legacy_orders_and_preserves_stars_and_caches() {
    let t = TestDb::new();
    let mut stale_account = signal(&t);
    let updated_at = stale_account.updated_at;
    assert!(!t.write(|tx| Account::set_default_room_order(tx, id("kevin"), &[id("hq")])));
    t.write(|tx| {
        tx.conn().execute(
            "UPDATE accounts SET settings = json_set(settings, '$.matchbox_channel_order', json(?))",
            [serde_json::json!({ id("kevin").to_string(): [id("hq")] }).to_string()],
        )?;
        Ok(())
    });
    assert!(t.write(|tx| Account::set_channel_favorite(tx, id("kevin"), id("hq"), true)).is_some());
    assert!(t.write(|tx| Account::set_default_room_order(tx, id("david"), &[id("pets"), id("hq")])));
    assert_eq!(signal(&t).updated_at, updated_at);
    t.write(move |tx| stale_account.update(tx, None, None, Some(&[("hide_translation_buttons", "false")])));
    let settings = signal(&t).settings();
    assert_eq!(settings.default_room_order(), vec![id("pets"), id("hq")]);
    assert!(settings.get("matchbox_channel_order").is_none());
    assert_eq!(settings.favorite_channels(id("kevin")), vec![id("hq")]);
    assert!(!settings.hide_translation_buttons());
    for ids in [vec![id("hq"), id("hq")], vec![id("david_and_kevin")], vec![0], vec![i64::MAX]] {
        assert!(!t.write(move |tx| Account::set_default_room_order(tx, id("david"), &ids)));
        assert_eq!(signal(&t).settings(), settings);
    }
    assert!(t.write(|tx| Account::set_default_room_order(tx, id("david"), &[])));
    assert!(signal(&t).settings().default_room_order().is_empty());
    assert_eq!(signal(&t).settings().favorite_channels(id("kevin")), vec![id("hq")]);
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
