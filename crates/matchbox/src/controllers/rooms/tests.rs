//! Request-level tests for the room controllers, against the `default` parity seed.

use axum::http::{Method, StatusCode};
use matchbox_db::{Membership, Room, RoomType};

use crate::controllers::presenters::test_support::*;

#[tokio::test]
async fn show_renders_the_room_and_remembers_it() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    let reply = david.get(&format!("/rooms/{ALL_TALK}")).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.content_type(), Some("text/html; charset=utf-8"));
    let html = reply.text();
    assert!(html.contains("<title>All Talk</title>"), "{html}");
    assert!(html.contains(r#"<meta name="current-room-id" content="486777696">"#));
    assert_eq!(html.matches(r#"data-controller="reply""#).count(), 40, "the last page");
    assert!(reply.headers.get_all("set-cookie").iter().any(|c| c.to_str().unwrap().starts_with(&format!("last_room={ALL_TALK}"))));
    assert_eq!(reply.header("x-version"), Some("parity"));
}

#[tokio::test]
async fn show_at_a_message_pages_around_it() {
    let Some(app) = TestApp::boot().await else { return };
    let first = app
        .db()
        .read(|conn| Ok(matchbox_db::Message::for_room(conn, ALL_TALK)?.into_iter().min_by_key(|m| m.created_at).unwrap()))
        .await
        .unwrap();
    let reply = app.david().get(&format!("/rooms/{ALL_TALK}/@{}", first.id)).await;
    assert_eq!(reply.status, StatusCode::OK);
    // The first message and the 40 after it.
    assert_eq!(reply.text().matches(r#"data-controller="reply""#).count(), 41);
}

#[tokio::test]
async fn inaccessible_rooms_redirect_home_with_an_alert() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    let reply = david.get(&format!("/rooms/{DIRECT_KEVIN_BENDER}")).await;
    assert_eq!(reply.status, StatusCode::FOUND);
    assert_eq!(reply.location(), Some("http://campfire.test/"));
    let reply = david.get("/rooms/nonsense").await;
    assert_eq!(reply.status, StatusCode::FOUND);
}

/// `@membership.room` is nil for a membership whose room is gone, and Rails fails on it: a 500,
/// not the 404 of a membership that isn't there.
#[tokio::test]
async fn a_membership_of_a_missing_room_is_a_server_error() {
    let Some(app) = TestApp::boot().await else { return };
    let missing_room = 999_999;
    app.db()
        .write(move |tx| {
            tx.conn().execute(
                "INSERT INTO memberships (room_id, user_id, created_at, updated_at) VALUES (?1, ?2, '2026-01-01', '2026-01-01')",
                [missing_room, DAVID],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let mut david = app.david();
    assert_eq!(david.get(&format!("/rooms/{missing_room}/messages")).await.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(david.get(&format!("/rooms/{}/messages", missing_room + 1)).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn index_redirects_to_the_last_room() {
    let Some(app) = TestApp::boot().await else { return };
    let reply = app.david().get("/rooms").await;
    assert_eq!(reply.status, StatusCode::FOUND);
    assert!(reply.location().unwrap().starts_with("http://campfire.test/rooms/"));
    // Anonymous: off to sign in.
    let reply = app.anonymous().get("/rooms").await;
    assert_eq!(reply.location(), Some("http://campfire.test/session/new"));
}

#[tokio::test]
async fn undeclared_actions() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    assert_eq!(david.get("/rooms/new").await.status, StatusCode::NOT_FOUND);
    assert_eq!(david.get(&format!("/rooms/{ALL_TALK}/edit")).await.status, StatusCode::NOT_FOUND);
    let direct = david.get(&format!("/rooms/directs/{DIRECT_DAVID_JASON}")).await;
    assert_eq!(direct.status, StatusCode::FOUND);
    assert!(direct.header("location").unwrap().ends_with(&format!("/rooms/{DIRECT_DAVID_JASON}")));
    let reply = david.write(Req::new(Method::DELETE, &format!("/rooms/opens/{HQ}"))).await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn open_rooms_are_created_edited_and_updated() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    let new = david.get("/rooms/opens/new").await;
    assert_eq!(new.status, StatusCode::OK);
    assert!(new.text().contains("New chat room"));

    let created = david.write(Req::new(Method::POST, "/rooms/opens").form(&[("room[name]", "Watercooler")])).await;
    assert_eq!(created.status, StatusCode::FOUND, "{}", created.text());
    let room_id: i64 = created.location().unwrap().rsplit('/').next().unwrap().parse().unwrap();
    let room = app.db().read(move |conn| Room::find(conn, room_id)).await.unwrap();
    assert_eq!((room.name.as_deref(), room.room_type), (Some("Watercooler"), RoomType::Open));
    // Rooms::Open grants every active user after commit.
    let members = app.db().read(move |conn| Membership::for_room(conn, room_id)).await.unwrap();
    assert!(members.len() > 3);

    assert_eq!(david.get(&format!("/rooms/opens/{room_id}/edit")).await.status, StatusCode::OK);
    let shown = david.get(&format!("/rooms/opens/{room_id}")).await;
    assert_eq!(shown.location(), Some(format!("http://campfire.test/rooms/{room_id}").as_str()));

    let updated = david
        .write(
            Req::new(Method::PATCH, &format!("/rooms/closeds/{room_id}"))
                .form(&[("room[name]", "Private"), ("user_ids[]", &DAVID.to_string())]),
        )
        .await;
    assert_eq!(updated.status, StatusCode::FOUND, "{}", updated.text());
    let room = app.db().read(move |conn| Room::find(conn, room_id)).await.unwrap();
    assert_eq!((room.name.as_deref(), room.room_type), (Some("Private"), RoomType::Closed));
    let members = app.db().read(move |conn| Membership::for_room(conn, room_id)).await.unwrap();
    assert_eq!(members.iter().map(|m| m.user_id).collect::<Vec<_>>(), vec![DAVID]);

    let missing_param = david.write(Req::new(Method::POST, "/rooms/opens").form(&[("name", "x")])).await;
    assert_eq!(missing_param.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn closed_rooms_are_created_with_the_selected_users() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    assert_eq!(david.get("/rooms/closeds/new").await.status, StatusCode::OK);
    let created = david
        .write(Req::new(Method::POST, "/rooms/closeds").form(&[
            ("room[name]", "Secret"),
            ("user_ids[]", &DAVID.to_string()),
            ("user_ids[]", &JASON.to_string()),
            ("user_ids[]", "999"),
        ]))
        .await;
    assert_eq!(created.status, StatusCode::FOUND, "{}", created.text());
    let room_id: i64 = created.location().unwrap().rsplit('/').next().unwrap().parse().unwrap();
    let mut members: Vec<i64> =
        app.db().read(move |conn| Membership::for_room(conn, room_id)).await.unwrap().iter().map(|m| m.user_id).collect();
    members.sort();
    assert_eq!(members, vec![DAVID, JASON]);
    assert_eq!(david.get(&format!("/rooms/closeds/{room_id}/edit")).await.status, StatusCode::OK);
}

#[tokio::test]
async fn only_administrators_or_creators_update_rooms() {
    let Some(app) = TestApp::boot().await else { return };
    // Jason (an administrator in the seed) isn't needed: David is an admin, so check the scope instead:
    // direct rooms are out of reach of the open/closed controllers.
    let mut david = app.david();
    let reply = david.get(&format!("/rooms/opens/{DIRECT_DAVID_JASON}/edit")).await;
    assert_eq!(reply.status, StatusCode::FOUND);
    assert_eq!(reply.location(), Some("http://campfire.test/"));
}

#[tokio::test]
async fn direct_rooms_are_found_or_created() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    assert_eq!(david.get("/rooms/directs/new").await.status, StatusCode::OK);
    let existing = david.write(Req::new(Method::POST, "/rooms/directs").form(&[("user_ids[]", &JASON.to_string())])).await;
    assert_eq!(existing.location(), Some(format!("http://campfire.test/rooms/{DIRECT_DAVID_JASON}").as_str()));
    let created = david.write(Req::new(Method::POST, "/rooms/directs").form(&[("user_ids[]", &KEVIN.to_string())])).await;
    let room_id: i64 = created.location().unwrap().rsplit('/').next().unwrap().parse().unwrap();
    let room = app.db().read(move |conn| Room::find(conn, room_id)).await.unwrap();
    assert_eq!(room.room_type, RoomType::Direct);

    assert_eq!(david.get(&format!("/rooms/directs/{room_id}/edit")).await.status, StatusCode::OK);
    let destroyed = david.write(Req::new(Method::DELETE, &format!("/rooms/directs/{room_id}"))).await;
    assert_eq!(destroyed.location(), Some("http://campfire.test/"));
    assert!(app.db().read(move |conn| Room::find_by_id(conn, room_id)).await.unwrap().is_none());
}

#[tokio::test]
async fn refresh_streams_messages_since_a_time() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    let reply = david
        .send(
            Req::new(Method::GET, &format!("/rooms/{ALL_TALK}/refresh?since=0")).header("accept", "text/vnd.turbo-stream.html, text/html"),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.content_type(), Some("text/vnd.turbo-stream.html; charset=utf-8"));
    assert!(reply.text().starts_with(r#"<turbo-stream action="append" target="messages_rooms_closed_486777696">"#), "{}", reply.text());

    let html_only = david.get(&format!("/rooms/{ALL_TALK}/refresh?since=0")).await;
    assert_eq!(html_only.status, StatusCode::NOT_ACCEPTABLE);
    assert_eq!(david.get(&format!("/rooms/{DIRECT_KEVIN_BENDER}/refresh")).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn involvement_is_shown_and_changed() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    let shown = david.get(&format!("/rooms/{ALL_TALK}/involvement")).await;
    assert_eq!(shown.status, StatusCode::OK);
    assert!(shown.text().contains("turbo-frame"));
    let updated =
        david.write(Req::new(Method::PATCH, &format!("/rooms/{ALL_TALK}/involvement")).form(&[("involvement", "invisible")])).await;
    assert_eq!(updated.location(), Some(format!("http://campfire.test/rooms/{ALL_TALK}/involvement").as_str()));
    let membership = app.db().read(|conn| Membership::find_by_room_and_user(conn, ALL_TALK, DAVID)).await.unwrap().unwrap();
    assert_eq!(membership.involvement, Some(matchbox_db::Involvement::Invisible));
    let invalid = david.write(Req::new(Method::PATCH, &format!("/rooms/{ALL_TALK}/involvement")).form(&[("involvement", "loud")])).await;
    assert_eq!(invalid.status, StatusCode::INTERNAL_SERVER_ERROR);

    // A missing (or blank) involvement is stored as nil, like the enum casts it.
    let missing = david.write(Req::new(Method::PATCH, &format!("/rooms/{ALL_TALK}/involvement"))).await;
    assert_eq!(missing.status, StatusCode::FOUND);
    let membership = app.db().read(|conn| Membership::find_by_room_and_user(conn, ALL_TALK, DAVID)).await.unwrap().unwrap();
    assert_eq!(membership.involvement, None);
}

#[tokio::test]
async fn rooms_are_destroyed_by_administrators() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    let reply = david.write(Req::new(Method::DELETE, &format!("/rooms/{QUIET_CORNER}"))).await;
    assert_eq!(reply.location(), Some("http://campfire.test/"));
    assert!(app.db().read(|conn| Room::find_by_id(conn, QUIET_CORNER)).await.unwrap().is_none());
}

#[tokio::test]
async fn cross_site_writes_are_refused() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    let request = Req::new(Method::POST, "/rooms/opens").form(&[("room[name]", "x")]).header("sec-fetch-site", "cross-site");
    assert_eq!(david.send(request).await.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn the_last_room_cookie_is_set_only_when_it_changes() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    let last_room =
        |reply: &Reply| reply.headers.get_all(axum::http::header::SET_COOKIE).iter().any(|c| c.to_str().unwrap().starts_with("last_room="));
    assert!(last_room(&david.get(&format!("/rooms/{HQ}")).await));
    assert!(!last_room(&david.get(&format!("/rooms/{HQ}")).await), "the same room again");
    assert!(last_room(&david.get(&format!("/rooms/{ALL_TALK}")).await));
}

/// Where a page's `link_back_to_last_room_visited` goes.
fn back_link(html: &str) -> &str {
    let link = &html[..html.find("Go Back</span></a>").expect("a back link")];
    let tag = &link[link.rfind("<a ").unwrap()..];
    let href = &tag[tag.find(r#"href=""#).unwrap() + 6..];
    &href[..href.find('"').unwrap()]
}

/// The layout's `last_room_visited`: the `last_room` cookie's room when the user is in it, else
/// their first room.
#[tokio::test]
async fn back_links_go_to_the_last_room_visited() {
    let Some(app) = TestApp::boot().await else { return };
    let original = app.db().read(|conn| Room::original_for_user(conn, DAVID)).await.unwrap().unwrap().id;
    assert_ne!(original, QUIET_CORNER);
    let mut david = app.david();
    let back_link_of = |reply: Reply| {
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
        back_link(&reply.text()).to_string()
    };

    assert_eq!(back_link_of(david.get("/rooms/opens/new").await), format!("/rooms/{original}"), "no cookie");
    david.get(&format!("/rooms/{QUIET_CORNER}")).await;
    assert_eq!(back_link_of(david.get("/rooms/opens/new").await), format!("/rooms/{QUIET_CORNER}"));
    assert_eq!(back_link_of(david.get("/account/edit").await), format!("/rooms/{QUIET_CORNER}"));
    david.set_cookie("last_room", &DIRECT_KEVIN_BENDER.to_string());
    assert_eq!(back_link_of(david.get("/rooms/opens/new").await), format!("/rooms/{original}"), "a room he isn't in");
    david.set_cookie("last_room", "nonsense");
    assert_eq!(back_link_of(david.get("/rooms/opens/new").await), format!("/rooms/{original}"));
}

#[tokio::test]
async fn a_room_page_has_the_same_etag_cold_and_warm() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    let etag = |reply: &Reply| reply.header("etag").map(str::to_string);
    // The first render stores the page's messages in the fragment cache; the second reads them.
    let cold = david.get(&format!("/rooms/{HQ}")).await;
    let warm = david.get(&format!("/rooms/{HQ}")).await;
    assert_eq!(cold.text(), warm.text());
    assert!(etag(&cold).is_some());
    assert_eq!(etag(&cold), etag(&warm));
}

/// A page of messages goes out in parts that are never joined: as they are to a client without
/// gzip, gzipped from their stored pieces, and as just its length for HEAD. It's the same page with
/// the same ETag every way (a room page, its Turbo-Frame version, and a page of older messages).
#[tokio::test]
async fn pages_of_messages_go_out_in_parts() {
    let Some(app) = TestApp::boot().await else { return };
    let mut david = app.david();
    let mut messages = app.db().read(|conn| matchbox_db::Message::for_room(conn, ALL_TALK)).await.unwrap();
    messages.sort_by_key(|m| (m.created_at, m.id));
    let pages = [
        (format!("/rooms/{ALL_TALK}"), None),
        (format!("/rooms/{ALL_TALK}"), Some("messages")),
        (format!("/rooms/{ALL_TALK}/messages?before={}", messages[60].id), None),
    ];
    for (path, frame) in &pages {
        let send = |method: Method, gzip: bool| {
            let mut request = Req::new(method, path);
            if gzip {
                request = request.header("accept-encoding", "gzip");
            }
            if let Some(frame) = frame {
                request = request.header("turbo-frame", frame);
            }
            request
        };
        let plain = david.send(send(Method::GET, false)).await;
        assert_eq!(plain.status, StatusCode::OK, "{path}");
        assert_eq!(plain.text().matches(r#"data-controller="reply""#).count(), 40, "{path}");
        assert!(plain.frames > 40, "{path}: {} frames, one per part", plain.frames);
        assert_eq!(plain.header("content-length"), Some(plain.body.len().to_string().as_str()), "{path}");

        let gzipped = david.send(send(Method::GET, true)).await;
        assert_eq!(gzipped.header("content-encoding"), Some("gzip"), "{path}");
        assert_eq!(gzipped.header("content-length"), None, "{path}");
        let mut decoded = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&gzipped.body[..]), &mut decoded).unwrap();
        assert!(decoded == plain.body, "{path}: gzip decodes to the plain page");

        for gzip in [false, true] {
            let head = david.send(send(Method::HEAD, gzip)).await;
            assert_eq!(head.status, StatusCode::OK, "{path}");
            assert_eq!(head.header("etag"), plain.header("etag"), "{path}");
            if !gzip {
                assert!(head.body.is_empty(), "{path}");
                assert_eq!(head.header("content-length"), plain.header("content-length"), "{path}");
            }
        }
        assert!(plain.header("etag").is_some_and(|etag| etag.starts_with("W/")), "{path}");
        assert_eq!(gzipped.header("etag"), plain.header("etag"), "{path}");
    }
}
