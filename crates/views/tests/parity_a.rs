//! DOM parity of the layout, session, account and user templates against golden renders from the
//! reference app
//! (`reference-tools/views/a/render.rb`). Each case rebuilds, from facts.json, the view-model a
//! Rust controller would pass, renders it, and compares normalized token streams.

mod support;

use askama::Template;
use matchbox_views::helpers as h;
use matchbox_views::*;
use support::dom::{diff, normalize_html};
use support::facts::*;

fn assert_parity(name: &str, ext: &str, rendered: String) {
    let expected = normalize_html(&golden(name, ext));
    let actual = normalize_html(&rendered);
    if let Some(report) = diff(&expected, &actual) {
        let out = golden_dir().join("../../../../../target/views-a/out");
        std::fs::create_dir_all(&out).ok();
        std::fs::write(out.join(format!("{name}.{ext}")), &rendered).ok();
        panic!("{name}: DOM differs from the reference\n{report}");
    }
}

fn help_contact(case: &str) -> Option<accounts::HelpContact> {
    let david = user(case, "David");
    Some(accounts::HelpContact {
        name: david["name"].as_str().unwrap().into(),
        email_address: david["email_address"].as_str().unwrap().into(),
    })
}

#[test]
fn sessions_new() {
    for (name, email, alert) in [
        ("sessions_new", None, None),
        ("sessions_new_email", Some("x@y.com"), None),
        ("sessions_new_rejected", Some("david@37signals.com"), Some("Too many requests or unauthorized.")),
        ("sessions_new_with_logo", None, None),
    ] {
        let request = Request { flash_alert: alert.map(Into::into), ..Default::default() };
        let html = with_context(name, request, |ctx| {
            sessions::New { ctx, email_address: email.map(Into::into), help_contact: help_contact(name) }.render().unwrap()
        });
        assert_parity(name, "html", html);
    }
}

#[test]
fn sessions_incompatible_browser() {
    for name in ["incompatible_browser", "incompatible_browser_apple_messages"] {
        let html = with_context(name, Request::default(), |ctx| sessions::IncompatibleBrowser { ctx }.render().unwrap());
        assert_parity(name, "html", html);
    }
}

#[test]
fn hidden_translation_controls_are_omitted_and_visibility_changes_reload_turbo() {
    with_context("sessions_new", Request { hide_translation_buttons: true, ..Default::default() }, |ctx| {
        let html = sessions::New { ctx, email_address: None, help_contact: help_contact("sessions_new") }.render().unwrap();
        assert!(!html.contains("language-list-menu"));
        assert!(!html.contains(">Translate<"));
        assert!(html.contains("name=\"matchbox-hide-translation-buttons\" content=\"true\" data-turbo-track=\"reload\""));
    });
    with_context("sessions_new", Request::default(), |ctx| {
        let html = sessions::New { ctx, email_address: None, help_contact: help_contact("sessions_new") }.render().unwrap();
        assert!(html.contains("language-list-menu"));
        assert!(!html.contains("matchbox-hide-translation-buttons"));
    });
}

#[test]
fn sessions_transfer() {
    let name = "sessions_transfer";
    let html = with_context(name, Request::default(), |ctx| {
        sessions::TransferShow { ctx, action: case(name)["path"].as_str().unwrap().into() }.render().unwrap()
    });
    assert_eq!(html.matches("<form ").count(), html.matches("</form>").count());
    let controller = "data-controller=\"auto-submit\"";
    assert_eq!(html.matches(controller).count(), 1);
    let start = html.find(controller).unwrap();
    let end = start + html[start..].find("</form>").unwrap();
    let form = &html[start..end];
    assert!(form.contains("name=\"_method\" value=\"put\""));
    assert!(!form.contains("<form "));
    // The pinned reference omits this closing tag. Require the complete form above,
    // then compare the rest of the page without changing historical fixtures.
    let mut legacy = html;
    legacy.replace_range(end..end + "</form>".len(), "");
    assert_parity(name, "html", legacy);
}

fn user_summary(value: &serde_json::Value) -> users::UserSummary {
    users::UserSummary {
        id: value["id"].as_i64().unwrap(),
        name: value["name"].as_str().unwrap().into(),
        bio: str_of(&value["bio"]),
        email_address: str_of(&value["email_address"]),
        role: match value["role"].as_str().unwrap() {
            "administrator" => users::Role::Administrator,
            "bot" => users::Role::Bot,
            _ => users::Role::Member,
        },
        status: match value["status"].as_str().unwrap() {
            "deactivated" => users::Status::Deactivated,
            "banned" => users::Status::Banned,
            _ => users::Status::Active,
        },
        avatar_path: value["avatar_path"].as_str().unwrap().into(),
    }
}

/// All users in case `name`, `User.ordered` (by lowercased name).
fn ordered_users(name: &str) -> Vec<users::UserSummary> {
    let mut all: Vec<_> = case(name)["users"].as_object().unwrap().values().map(user_summary).collect();
    all.sort_by_key(|user| user.name.to_lowercase());
    all
}

fn account_fact(name: &str, key: &str) -> serde_json::Value {
    case(name)["account"][key].clone()
}

#[test]
fn accounts_edit() {
    for (name, notice) in [
        ("account_edit_admin", None),
        ("account_edit_member", None),
        ("account_edit_notice", Some("✓")),
        ("account_edit_paginated", None),
        ("account_edit_with_logo", None),
        ("account_edit_with_logo_member", None),
    ] {
        let request = Request { flash_notice: notice.map(Into::into), ..Default::default() };
        let html = with_context(name, request, |ctx| {
            // AccountsController#account_users
            let visible: Vec<_> = ordered_users(name)
                .into_iter()
                .filter(|user| !user.bot())
                .filter(|user| if ctx.can_administer() { user.active() || user.banned() } else { user.active() })
                .collect();
            let (administrators, members) = visible.into_iter().partition(|user| user.administrator());
            accounts::Edit {
                ctx,
                account_id: account_fact(name, "id").as_i64().unwrap(),
                join_code: account_fact(name, "join_code").as_str().unwrap().into(),
                restrict_room_creation_to_administrators: account_fact(name, "restrict_room_creation_to_administrators").as_bool().unwrap(),
                administrators,
                members,
                next_page: (name == "account_edit_paginated").then(|| "2".to_string()),
            }
            .render()
            .unwrap()
        });
        // Compare the original page outside the added, separately exercised admin control.
        let mut legacy = html;
        if let Some(start) = legacy.find("    <!-- Matchbox localization setting -->") {
            let marker = "    <!-- /Matchbox localization setting -->\n";
            let end = legacy[start..].find(marker).unwrap() + start + marker.len();
            assert!(legacy[start..end].contains("account[settings][hide_translation_buttons]"));
            legacy.replace_range(start..end, "");
        } else {
            assert!(matches!(name, "account_edit_member" | "account_edit_with_logo_member"));
        }
        assert_parity(name, "html", legacy);
    }
}

fn bots(name: &str) -> Vec<accounts::Bot> {
    let facts = facts();
    ordered_users(name)
        .into_iter()
        .filter(|user| user.bot() && user.active())
        .map(|user| {
            let bot_key = user_by_email_or_name(name, &user.name)["bot_key"].as_str().unwrap().to_string();
            // bot.rooms.without_directs.ordered
            let mut rooms: Vec<accounts::BotRoom> = facts["memberships"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["user_id"].as_i64() == Some(user.id))
                .filter_map(|m| facts["rooms"].as_object().unwrap().values().find(|room| room["id"] == m["room_id"]))
                .filter(|room| room["type"] != "Rooms::Direct")
                .map(|room| accounts::BotRoom { id: room["id"].as_i64().unwrap(), name: room["name"].as_str().unwrap().into() })
                .collect();
            rooms.sort_by_key(|room| room.name.to_lowercase());
            accounts::Bot { user, bot_key, rooms }
        })
        .collect()
}

fn user_by_email_or_name(case_name: &str, name: &str) -> &'static serde_json::Value {
    user(case_name, name)
}

#[test]
fn accounts_bots() {
    let name = "bots_index";
    let html = with_context(name, Request::default(), |ctx| accounts::BotsIndex { ctx, bots: bots(name) }.render().unwrap());
    assert_parity(name, "html", html);

    let name = "bots_new";
    let html = with_context(name, Request::default(), |ctx| accounts::BotsNew { ctx, bot: accounts::BotForm::default() }.render().unwrap());
    assert_parity(name, "html", html);

    let name = "bots_edit";
    let bender = user(name, "Bender Bot");
    let html = with_context(name, Request::default(), |ctx| {
        accounts::BotsEdit {
            ctx,
            bot_id: bender["id"].as_i64().unwrap(),
            bot: accounts::BotForm {
                name: str_of(&bender["name"]),
                webhook_url: str_of(&bender["webhook_url"]),
                avatar_attachment_url: None,
            },
        }
        .render()
        .unwrap()
    });
    assert_parity(name, "html", html);

    let name = "bots_edit_with_avatar";
    let bender = user(name, "Bender Bot");
    let html = with_context(name, Request::default(), |ctx| {
        accounts::BotsEdit {
            ctx,
            bot_id: bender["id"].as_i64().unwrap(),
            bot: accounts::BotForm {
                name: str_of(&bender["name"]),
                webhook_url: str_of(&bender["webhook_url"]),
                avatar_attachment_url: str_of(&case(name)["avatar_url"]),
            },
        }
        .render()
        .unwrap()
    });
    assert_parity(name, "html", html);
}

#[test]
fn accounts_custom_styles() {
    for name in ["custom_styles_edit", "custom_styles_layout"] {
        let html = with_context(name, Request::default(), |ctx| {
            accounts::CustomStylesEdit { ctx, custom_styles: str_of(&account_fact(name, "custom_styles")) }.render().unwrap()
        });
        assert_parity(name, "html", html);
    }
}

fn data(name: &str) -> &'static serde_json::Value {
    &case(name)["data"]
}

fn named(name: &str, user_name: &str) -> users::UserSummary {
    user_summary(user(name, user_name))
}

fn mention_user(name: &str, user_name: &str) -> users::MentionUser {
    users::MentionUser { user: named(name, user_name), attachable_sgid: user(name, user_name)["attachable_sgid"].as_str().unwrap().into() }
}

#[test]
fn first_runs_show() {
    // KRO intentionally changes the setup presentation. Keep the Rails form contract instead
    // of masking this screen out of all verification or rewriting its historical golden HTML.
    let html = with_context("first_run", Request { basecoat: true, ..Default::default() }, |ctx| {
        first_runs::Show { ctx, values: first_runs::FormValues::default(), reload_frame: false }.render().unwrap()
    });
    let tokens = normalize_html(&html);
    let form = tokens.iter().find(|token| token.starts_with("<form ")).unwrap();
    for attribute in ["action=\"/first_run\"", "method=\"post\"", "enctype=\"multipart/form-data\"", "accept-charset=\"UTF-8\""] {
        assert!(form.contains(attribute), "form missing {attribute}");
    }
    for (name, field_type, autocomplete) in
        [("name", "text", "name"), ("email_address", "email", "username"), ("password", "password", "new-password")]
    {
        let input = tokens.iter().find(|token| token.starts_with("<input ") && token.contains(&format!("name=\"user[{name}]\""))).unwrap();
        for attribute in [
            format!("type=\"{field_type}\""),
            format!("id=\"user_{name}\""),
            format!("autocomplete=\"{autocomplete}\""),
            "required=\"required\"".into(),
        ] {
            assert!(input.contains(&attribute), "{name} missing {attribute}");
        }
        assert!(tokens.iter().any(|token| token == &format!("<label for=\"user_{name}\">")));
    }
    let avatar = tokens.iter().find(|token| token.starts_with("<input ") && token.contains("name=\"user[avatar]\"")).unwrap();
    for attribute in
        ["type=\"file\"", "accept=\"image/*\"", "data-upload-preview-target=\"input\"", "data-action=\"upload-preview#previewImage\""]
    {
        assert!(avatar.contains(attribute), "avatar missing {attribute}");
    }
    assert!(!avatar.contains("required="));
    assert!(html.contains("maxlength=\"72\""));
    assert!(html.contains("minlength=\"8\""));
    assert!(html.contains("autofocus=\"autofocus\""));
    assert!(html.contains("data-1p-ignore=\"true\""));
    assert!(html.contains("Continue"));
    assert!(html.contains("Set up Matchbox"));
    assert!(!html.contains("data-controller=\"popup\""));
    assert!(html.contains("data-password-toggle"));
    assert!(!html.contains("data-appearance-control"));
    assert!(!html.contains("data-appearance-select"));
    assert!(html.contains("data-style-profile=\"basecoat\""));
    assert!(html.contains("initial-scale=1, interactive-widget=resizes-content"));
    assert!(!html.contains("user-scalable=no"));
    assert!(html.find("basecoat/theme-init.js").unwrap() < html.find("basecoat/app.css").unwrap());
    assert!(!html.contains("turbo-visit-control"));
}

#[test]
fn first_runs_frames_promote_only_when_crossing_stylesheet_profiles() {
    with_context("first_run", Request { basecoat: true, ..Default::default() }, |ctx| {
        for reload_frame in [false, true] {
            let page = first_runs::Show { ctx, values: first_runs::FormValues::default(), reload_frame };
            let html = layouts::frame(ctx, page.as_head(), page.as_content()).unwrap();
            assert_eq!(html.text().contains("name=\"turbo-visit-control\" content=\"reload\""), reload_frame);
            assert!(html.text().contains("name=\"user[email_address]\""));
            assert!(!html.text().contains("basecoat/app.js"));
        }
    });
}

#[test]
fn first_runs_layout_escapes_flash_and_preserves_application_metadata() {
    let html = with_context(
        "first_run",
        Request { basecoat: true, flash_alert: Some("<script>alert('bad')</script>".into()), ..Default::default() },
        |ctx| first_runs::Show { ctx, values: first_runs::FormValues::default(), reload_frame: false }.render().unwrap(),
    );
    assert!(html.contains("role=\"alert\" aria-atomic=\"true\""));
    assert!(html.contains("&lt;script&gt;alert(&#39;bad&#39;)&lt;/script&gt;"));
    for expected in
        ["action-cable-url", "vapid-public-key", "webmanifest.json", "apple-touch-icon", "type=\"importmap\"", "Skip to main content"]
    {
        assert!(html.contains(expected), "missing {expected}");
    }
}

#[test]
fn users_new() {
    let name = "users_new";
    let html = with_context(name, Request::default(), |ctx| {
        users::New { ctx, join_code: account_fact(name, "join_code").as_str().unwrap().into(), help_contact: help_contact(name) }
            .render()
            .unwrap()
    });
    assert_parity(name, "html", html);
}

#[test]
fn users_show() {
    for (name, shown) in [
        ("users_show_self", "David"),
        ("users_show_member_as_admin", "JZ"),
        ("users_show_member_as_member", "JZ"),
        ("users_show_bot", "Bender Bot"),
        ("users_show_deactivated", "Ex Employee"),
        ("users_show_banned", "Spam Ham"),
    ] {
        let html = with_context(name, Request::default(), |ctx| {
            users::Show { ctx, user: named(name, shown), transfer_id: user(name, shown)["transfer_id"].as_str().unwrap().into() }
                .render()
                .unwrap()
        });
        assert_parity(name, "html", html);
    }
}

fn profile_memberships(list: &serde_json::Value) -> Vec<users::ProfileMembership> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|m| users::ProfileMembership {
            room_id: m["room_id"].as_i64().unwrap(),
            room_param_key: m["param_key"].as_str().unwrap().into(),
            room_display_name: m["display_name"].as_str().unwrap().into(),
            involvement: m["involvement"].as_str().unwrap().into(),
            direct: m["direct"].as_bool().unwrap(),
        })
        .collect()
}

#[test]
fn users_profiles_show() {
    for ua in [
        "chrome_mac", "chrome_windows", "safari_mac", "safari_ios", "chrome_android", "firefox_mac", "firefox_android", "edge_windows",
        "kevin", "with_avatar",
    ] {
        let name = format!("profile_{ua}");
        let name = name.as_str();
        let html = with_context(name, Request::default(), |ctx| {
            let me = user_by_email(name, case(name)["as"].as_str().unwrap());
            users::ProfileShow {
                ctx,
                user: user_summary(me),
                avatar_attached: me["avatar_attached"].as_bool().unwrap(),
                transfer_id: me["transfer_id"].as_str().unwrap().into(),
                shared_memberships: profile_memberships(&data(name)["profile"]["shared"]),
                direct_memberships: profile_memberships(&data(name)["profile"]["direct"]),
            }
            .render()
            .unwrap()
        });
        assert_parity(name, "html", html);
    }
}

#[test]
fn users_push_subscriptions() {
    let name = "push_subscriptions";
    let html = with_context(name, Request::default(), |ctx| {
        let push_subscriptions = data(name)["push_subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|ps| users::PushSubscription {
                id: ps["id"].as_i64().unwrap(),
                endpoint: ps["endpoint"].as_str().unwrap().into(),
                browser: ps["browser"].as_str().unwrap().into(),
                version: ps["version"].as_str().unwrap().into(),
                platform: ps["platform"].as_str().unwrap().into(),
            })
            .collect();
        users::PushSubscriptionsIndex { ctx, push_subscriptions }.render().unwrap()
    });
    assert_parity(name, "html", html);
}

fn sidebar<'a>(name: &str, ctx: &'a matchbox_views::ViewContext<'a>) -> users::SidebarShow<'a> {
    let sidebar = &data(name)["sidebar"];
    let me = user_by_email(name, case(name)["as"].as_str().unwrap());
    let me_name = me["name"].as_str().unwrap();
    users::SidebarShow {
        ctx,
        current_user: user_summary(me),
        rooms_stream: facts()["signed_streams"]["rooms"].as_str().unwrap().into(),
        user_rooms_stream: facts()["signed_streams"]["user_rooms"][me_name].as_str().unwrap().into(),
        direct_memberships: sidebar["direct"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| users::SidebarDirect {
                room_id: d["room_id"].as_i64().unwrap(),
                unread: d["unread"].as_bool().unwrap(),
                updated_at_epoch: d["updated_at_epoch"].as_str().unwrap().into(),
                members: d["member_names"].as_array().unwrap().iter().map(|n| named(name, n.as_str().unwrap())).collect(),
                membership_id: d["room_id"].as_i64().unwrap(),
                membership_updated_at: jiff::Timestamp::UNIX_EPOCH,
            })
            .map(users::SidebarDirectItem::from)
            .collect(),
        direct_placeholder_users: sidebar["placeholders"].as_array().unwrap().iter().map(|n| named(name, n.as_str().unwrap())).collect(),
        other_memberships: sidebar["shared"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| users::SidebarRoom {
                id: r["room_id"].as_i64().unwrap(),
                param_key: r["param_key"].as_str().unwrap().into(),
                name: r["name"].as_str().unwrap().into(),
                unread: r["unread"].as_bool().unwrap(),
            })
            .collect(),
        can_create_rooms: sidebar["can_create_rooms"].as_bool().unwrap(),
    }
}

#[test]
fn users_sidebars_show() {
    for name in ["sidebar_david", "sidebar_kevin"] {
        let html = with_context(name, Request::default(), |ctx| sidebar(name, ctx).render().unwrap());
        assert_parity(name, "html", html);
    }
    let name = "sidebar_frame";
    let html = with_context(name, Request::default(), |ctx| {
        let page = sidebar(name, ctx);
        layouts::frame(ctx, page.as_head(), page.as_content()).unwrap().to_string()
    });
    assert_parity(name, "html", html);
}

#[test]
fn autocompletable_users() {
    let name = "autocompletable_users";
    let html = with_context(name, Request::default(), |ctx| {
        let users = data(name)["autocompletable"].as_array().unwrap().iter().map(|n| mention_user(name, n.as_str().unwrap())).collect();
        autocompletable::UsersIndex { ctx, users }.render().unwrap()
    });
    assert_parity(name, "html", html);

    let name = "autocompletable_users_json";
    let users: Vec<_> = data(name)["autocompletable"].as_array().unwrap().iter().map(|n| mention_user(name, n.as_str().unwrap())).collect();
    let json = autocompletable::users_index_json(&users, facts()["base_url"].as_str().unwrap());
    assert_eq!(json, golden(name, "json"));
}

#[test]
fn users_avatars_show() {
    for (name, shown) in [("avatar_david", "David"), ("avatar_three_initials", "Anna Bea Cole")] {
        let user = user(name, shown);
        let svg = users::AvatarSvg { user_id: user["id"].as_i64().unwrap(), initials: user["initials"].as_str().unwrap().into() }
            .render()
            .unwrap();
        assert_parity(name, "svg", svg.clone());
        assert_eq!(svg, golden(name, "svg"), "{name}: bytes differ");
    }
}

#[test]
fn pwa_manifest_and_service_worker() {
    let name = "manifest";
    let assets = facts()["assets"].as_object().unwrap();
    let asset_path = |logical: &str| assets[logical].as_str().unwrap().to_string();
    let json = pwa::Manifest {
        account_name: str_of(&account_fact(name, "name")),
        logo_path_small: account_fact(name, "logo_path_small").as_str().unwrap().into(),
        logo_path: account_fact(name, "logo_path").as_str().unwrap().into(),
        base_url: facts()["base_url"].as_str().unwrap().into(),
        asset_path: &asset_path,
    }
    .render()
    .unwrap();
    // Rails HTML-escapes the values into the JSON (README, Known differences)
    assert_eq!(json, golden(name, "json").replace("&amp;", "&"));
    assert_eq!(pwa::SERVICE_WORKER_JS, golden("service_worker", "js"));
}

#[test]
fn pwa_manifest_is_valid_json_whatever_the_account_is_called() {
    let asset_path = |logical: &str| format!("/assets/{logical}");
    let name = r#"Back\slash "quoted" <b>&amp;</b>"#;
    let json = pwa::Manifest {
        account_name: Some(name.into()),
        logo_path_small: "/account/logo?size=small&v=1".into(),
        logo_path: "/account/logo?v=1".into(),
        base_url: "http://campfire.test".into(),
        asset_path: &asset_path,
    }
    .render()
    .unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert_eq!(manifest["name"], name);
    assert_eq!(manifest["icons"][0]["src"], "/account/logo?size=small&v=1");
}

#[test]
fn welcome_show() {
    let name = "welcome";
    let html = with_context(name, Request::default(), |ctx| {
        welcome::Show { ctx, current_user_name: ctx.current_user.as_ref().unwrap().name.clone() }.render().unwrap()
    });
    assert_parity(name, "html", html);
}

#[test]
fn accounts_users_index_turbo_stream() {
    let name = "account_users_page_2";
    let html = with_context(name, Request::default(), |ctx| {
        let users = case(name)["page_users"].as_array().unwrap().iter().map(|n| named(name, n.as_str().unwrap())).collect();
        accounts::UsersIndexTurboStream { ctx, users, next_page: None }.render().unwrap()
    });
    assert_parity(name, "turbo_stream.html", html);
}

#[test]
fn pwa_partials() {
    for ua in facts()["user_agents"].as_object().unwrap().keys() {
        for kind in ["browser_settings", "system_settings", "install_instructions"] {
            let name = format!("{kind}_{ua}");
            let name = name.as_str();
            let html = with_context(name, Request::default(), |ctx| match kind {
                "browser_settings" => pwa::BrowserSettings { ctx }.render().unwrap(),
                "system_settings" => pwa::SystemSettings { ctx }.render().unwrap(),
                _ => pwa::InstallInstructions { ctx }.render().unwrap(),
            });
            assert_parity(name, "html", html);
        }
    }
}

#[test]
fn users_partials() {
    let name = "autocompletables_template";
    let html = with_context(name, Request::default(), |ctx| users::AutocompletableTemplate { ctx }.render().unwrap());
    assert_parity(name, "html", html);

    let name = "mention";
    let html = with_context(name, Request::default(), |ctx| users::Mention { ctx, user: mention_user(name, "JZ") }.render().unwrap());
    assert_parity(name, "html", html);

    let name = "ban_button_banned";
    let html = with_context(name, Request { partial: true, ..Default::default() }, |ctx| {
        users::BanButton { ctx, user: named(name, "Spam Ham") }.render().unwrap()
    });
    assert_parity(name, "html", html);

    for (name, room_name, unread) in [("shared_room_unread", "HQ", true), ("shared_room", "All Talk", false)] {
        let room = &facts()["rooms"][room_name];
        let html = users::SidebarSharedPartial {
            room: users::SidebarRoom {
                id: room["id"].as_i64().unwrap(),
                param_key: room["param_key"].as_str().unwrap().into(),
                name: room_name.into(),
                unread,
            },
        }
        .render()
        .unwrap();
        assert_parity(name, "html", html);
    }

    let name = "user_json";
    let jz = user(name, "JZ");
    let json = rails_compat::json::encode(&matchbox_views::messages::json::UserJson {
        id: jz["id"].as_i64().unwrap(),
        name: "JZ".into(),
        role: jz["role"].as_str().unwrap().into(),
        avatar_url: format!("{}{}", facts()["base_url"].as_str().unwrap(), jz["avatar_path"].as_str().unwrap()),
    });
    assert_eq!(json, golden(name, "json"));
}

#[test]
fn application_layout_wrapper_matches_extended_pages() {
    // A page rendered through layouts::Application with its parts equals the page extending the
    // layout directly.
    let name = "users_show_self";
    with_context(name, Request::default(), |ctx| {
        let transfer_id = user(name, "David")["transfer_id"].as_str().unwrap().to_string();
        let page = users::Show { ctx, user: named(name, "David"), transfer_id };
        let wrapped = layouts::Application {
            page_title: Some("David".into()),
            nav: h::raw(page.as_nav().render().unwrap()),
            content: h::raw(page.as_content().render().unwrap()),
            ..layouts::Application::new(ctx, h::empty())
        }
        .render()
        .unwrap();
        assert_parity(name, "html", wrapped);
    });
}
