//! DOM parity of the message views with the reference app (goldens in tests/golden/b).

mod messages_support;

use askama::Template;
use ember_views::messages::{self, EditView, MessageView, RoomKind, UserView};
use messages_support::golden;

#[test]
fn show_text_message() {
    let g = golden("messages_show_text");
    let message: MessageView = g.input();
    g.assert_content(&g.render(|ctx| messages::Show { ctx, message: &message }.render().unwrap()));
}

/// The partial is cached once for every request, so nothing in it may come from the request's
/// Host header (README, Known differences).
#[test]
fn message_partial_is_the_same_on_every_host() {
    let mut g = golden("messages_show_text");
    let message: MessageView = g.input();
    let render = |g: &messages_support::Golden| g.render(|ctx| messages::MessagePartial { ctx, message: &message }.render().unwrap());
    let on_the_reference_host = render(&g);
    g.json["context"]["base_url"] = "https://evil.example".into();
    assert_eq!(render(&g), on_the_reference_host);
    assert!(on_the_reference_host.contains(&format!("data-copy-to-clipboard-url-value=\"/rooms/{}/@{}\"", message.room_id, message.id)));
}

#[test]
fn compact_actions_keep_all_reactions_and_existing_message_endpoints() {
    for name in ["messages_show_text", "messages_show_image"] {
        let g = golden(name);
        let message: MessageView = g.input();
        let html = g.render(|ctx| messages::MessagePartial { ctx, message: &message }.render().unwrap());
        assert_eq!(html.matches("name=\"boost[content]\"").count(), messages::REACTIONS.len());
        for (character, _) in messages::REACTIONS {
            assert!(html.contains(&format!("value=\"{character}\"")));
        }
        assert_eq!(html.matches(&format!("action=\"{}\"", message.boosts_path())).count(), messages::REACTIONS.len());
        assert!(html.contains(&format!("data-turbo-frame=\"{}\"", message.dom_id("new_boost"))));
        assert!(html.contains(&format!("href=\"{}\"", message.edit_path())));
        assert!(html.contains(&format!("data-turbo-frame=\"{}\"", message.dom_id("edit"))));
        assert!(html.contains("lucide-face-slightly-smiling-plus"));
        assert_eq!(html.contains("data-action=\"reply#reply message-actions#close\""), message.attachment().is_none());
    }
}

#[test]
fn show_image_message() {
    let g = golden("messages_show_image");
    let message: MessageView = g.input();
    g.assert_content(&g.render(|ctx| messages::Show { ctx, message: &message }.render().unwrap()));
}

#[test]
fn svg_preview_keeps_download_fallback_and_escapes_metadata() {
    let g = golden("messages_show_image");
    let mut message: MessageView = g.input();
    let messages::MessageContent::Attachment(attachment) = &mut message.content else { panic!("expected attachment fixture") };
    attachment.preview = messages::AttachmentPreview::Svg;
    attachment.filename = "drawing\"<script>.svg".into();
    let rendered = g.render(|ctx| messages::MessagePartial { ctx, message: &message }.render().unwrap());
    assert!(rendered.contains("data-controller=\"svg-preview\""));
    assert!(rendered.contains("data-svg-preview-target=\"image\""));
    assert!(rendered.contains("drawing&quot;&lt;script&gt;.svg"));
    assert!(rendered.contains("Download drawing&quot;&lt;script&gt;.svg"));
    assert!(!rendered.contains("<script>"));
    assert!(!rendered.contains("<object"));
}

#[test]
fn index() {
    let g = golden("messages_index");
    let messages: Vec<messages::MessageItem> = g.input_at("messages");
    g.assert_content(&g.render(|ctx| messages::Index { ctx, messages: &messages }.render().unwrap()));
}

#[test]
fn edit_text_message() {
    let g = golden("messages_edit_text");
    let edit: EditView = g.input();
    g.assert_content(&g.render(|ctx| messages::Edit { ctx, edit: &edit }.render().unwrap()));
}

#[test]
fn edit_attachment_message() {
    let g = golden("messages_edit_attachment");
    let edit: EditView = g.input();
    g.assert_content(&g.render(|ctx| messages::Edit { ctx, edit: &edit }.render().unwrap()));
}

#[test]
fn create_stream() {
    let g = golden("messages_create");
    let message: messages::MessageItem = g.input_at("message");
    let room_kind: RoomKind = g.input_at("room_kind");
    g.assert_dom(&g.render(|ctx| messages::CreateStream { ctx, message: &message, room_kind }.render().unwrap()));
}

#[test]
fn destroy_stream() {
    let g = golden("messages_destroy");
    let message: MessageView = g.input();
    g.assert_dom(&g.render(|_| messages::DestroyStream { message: &message }.render().unwrap()));
}

#[test]
fn room_not_found() {
    let g = golden("messages_room_not_found");
    g.assert_content(&messages::RoomNotFound.render().unwrap());
}

#[test]
fn boosts_index() {
    let g = golden("messages_boosts_index");
    let message: MessageView = g.input();
    g.assert_content(&g.render(|ctx| messages::BoostsIndex { ctx, message: &message }.render().unwrap()));
}

#[test]
fn new_boost() {
    let g = golden("messages_boosts_new");
    let message: MessageView = g.input_at("message");
    let user: UserView = g.input_at("user");
    g.assert_content(&g.render(|ctx| messages::NewBoost { ctx, message: &message, user: &user }.render().unwrap()));
}

#[test]
fn by_bots_index_json() {
    let g = golden("messages_by_bots_index");
    let input: Vec<messages::json::MessageJson> = g.input();
    assert_eq!(messages::json::by_bots_index(&input), g.json["raw"].as_str().unwrap());
}

#[test]
fn by_bots_show_json() {
    let g = golden("messages_by_bots_show");
    let input: messages::json::MessageJson = g.input();
    assert_eq!(messages::json::by_bots_show(&input), g.json["raw"].as_str().unwrap());
}

#[test]
fn boosts_by_bots_show_json() {
    let g = golden("messages_boosts_by_bots_show");
    let input: messages::json::BoostJson = g.input();
    assert_eq!(messages::json::boosts_by_bots_show(&input), g.json["raw"].as_str().unwrap());
}
