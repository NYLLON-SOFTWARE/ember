//! Broadcasting names: `Channel::Naming#channel_name`, `Channel::Broadcasting#broadcasting_for`
//! and Turbo's `stream_name_from`.
//!
//! Callers pass each streamable already converted with `to_gid_param` (records, see
//! [`gid_param`]) or `to_param` (symbols and strings, which are themselves).
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rails_compat::global_id::GlobalId;

/// `GlobalID#to_param`: the URL-safe, unpadded Base64 of `gid://app/Model/id`. Records use their
/// STI class name, e.g. `Rooms::Open`.
pub fn gid_param(gid: &GlobalId) -> String {
    URL_SAFE_NO_PAD.encode(format!("gid://{}/{}/{}", gid.app, gid.model_name, gid.id))
}

/// `ActionCable::Channel::Naming.channel_name`:
/// `name.delete_suffix("Channel").gsub("::", ":").underscore`.
pub fn channel_name(class_name: &str) -> String {
    underscore(&class_name.strip_suffix("Channel").unwrap_or(class_name).replace("::", ":"))
}

/// `Channel::Broadcasting.broadcasting_for`: the channel name followed by each broadcastable,
/// joined with `:`. `stream_for @room` in `RoomChannel` streams from `room:<gid param>`.
pub fn broadcasting_for(class_name: &str, broadcastables: &[&str]) -> String {
    std::iter::once(channel_name(class_name).as_str()).chain(broadcastables.iter().copied()).collect::<Vec<_>>().join(":")
}

/// `Turbo::Streams::StreamName#stream_name_from`: `turbo_stream_from @room, :messages` names the
/// stream `<room gid param>:messages`.
pub fn stream_name_from(streamables: &[&str]) -> String {
    streamables.join(":")
}

/// `ActiveSupport::Inflector.underscore` without acronym inflections (Matchbox defines none).
fn underscore(word: &str) -> String {
    let chars: Vec<char> = word.replace("::", "/").chars().collect();
    let mut out = String::with_capacity(chars.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next = chars.get(i + 1).copied();
            // ([a-z\d])([A-Z]) and ([A-Z\d]+)([A-Z][a-z])
            let lower_before = prev.is_ascii_lowercase() || prev.is_ascii_digit();
            let acronym_end = (prev.is_ascii_uppercase() || prev.is_ascii_digit()) && next.is_some_and(|n| n.is_ascii_lowercase());
            if lower_before || acronym_end {
                out.push('_');
            }
        }
        out.push(if c == '-' { '_' } else { c.to_ascii_lowercase() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names() {
        assert_eq!(channel_name("RoomChannel"), "room");
        assert_eq!(channel_name("TypingNotificationsChannel"), "typing_notifications");
        assert_eq!(channel_name("Turbo::StreamsChannel"), "turbo:streams");
        assert_eq!(channel_name("HTMLChannel"), "html");
        assert_eq!(channel_name("HTMLParserChannel"), "html_parser");
    }

    #[test]
    fn broadcastings() {
        let room = gid_param(&GlobalId { app: "campfire".into(), model_name: "Rooms::Open".into(), id: "1".into() });
        assert_eq!(room, "Z2lkOi8vY2FtcGZpcmUvUm9vbXM6Ok9wZW4vMQ");
        assert_eq!(broadcasting_for("PresenceChannel", &[&room]), format!("presence:{room}"));
        assert_eq!(stream_name_from(&[&room, "messages"]), format!("{room}:messages"));
    }
}
