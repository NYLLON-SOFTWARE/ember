//! URL building that `matchbox_routes` leaves to the caller: query strings (`Hash#to_query`)
//! and format extensions (`path(format: :json)`).

/// `CGI.escape`, for templates too.
pub use ruby_compat::cgi_escape;

/// A query parameter value: a scalar or an array (`key[]=a&key[]=b`).
pub enum Param {
    One(String),
    Many(Vec<String>),
}

/// `url_for`'s extra params: `Hash#to_query`, which sorts by key.
pub fn with_query(path: &str, params: Vec<(&str, Param)>) -> String {
    let mut params = params;
    params.sort_by(|a, b| a.0.cmp(b.0));
    let mut pairs = Vec::new();
    for (key, value) in params {
        match value {
            Param::One(value) => pairs.push(format!("{}={}", cgi_escape(key), cgi_escape(&value))),
            Param::Many(values) => {
                let key = cgi_escape(&format!("{key}[]"));
                pairs.extend(values.iter().map(|value| format!("{key}={}", cgi_escape(value))));
            }
        }
    }
    if pairs.is_empty() { path.to_string() } else { format!("{path}?{}", pairs.join("&")) }
}

/// `rooms_directs_path(user_ids: [ id ])`.
pub fn rooms_directs_with_users(user_ids: &[i64]) -> String {
    with_query(&matchbox_routes::rooms_directs(), vec![("user_ids", Param::Many(user_ids.iter().map(ToString::to_string).collect()))])
}

/// `rooms_directs_path(user_ids: [ user.id ])`.
pub fn rooms_directs_with_user(user_id: impl std::borrow::Borrow<i64>) -> String {
    rooms_directs_with_users(&[*user_id.borrow()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_rails_query_strings() {
        assert_eq!(rooms_directs_with_users(&[5, 6]), "/rooms/directs?user_ids%5B%5D=5&user_ids%5B%5D=6");
        assert_eq!(with_query("/x", vec![("z", Param::One("a b".into())), ("a", Param::One("1".into()))]), "/x?a=1&z=a+b");
    }
}
