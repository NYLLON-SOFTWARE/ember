//! Replays testdata/opengraph_cases.json (fake DNS, a fake server for the fake public addresses)
//! and compares the response, the DNS lookups and the HTTP requests with what the reference
//! did for the same cases (testdata/opengraph_expected.json, from testdata/oracle/opengraph.rb).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::Value;

use super::*;
use crate::integrations::test_support::{FakeResolver, FakeServer, MappingDialer, Route, gzip_bomb, network, trickling_server};

fn route(spec: &Value) -> Route {
    let s = |key: &str| spec[key].as_str().unwrap_or_default().to_string();
    let mut route = Route::new(&s("method"), &s("host"), &s("path"), spec["status"].as_u64().unwrap() as u16);
    route.headers = serde_json::from_value(spec["headers"].clone()).unwrap();
    let mut body = if let Some(b64) = spec["body_b64"].as_str() {
        base64::engine::general_purpose::STANDARD.decode(b64).unwrap()
    } else if let Some(repeat) = spec["body_repeat"].as_array() {
        repeat[0].as_str().unwrap().repeat(repeat[1].as_u64().unwrap() as usize).into_bytes()
    } else {
        s("body").into_bytes()
    };
    if let Some(pad_to) = spec["pad_to"].as_u64() {
        body.resize(pad_to as usize, b' ');
    }
    route.body = body;
    route.chunked = spec["chunked"].as_bool().unwrap_or(false);
    route.gzip = spec["gzip"].as_bool().unwrap_or(false);
    route
}

fn answers(spec: &Value) -> Vec<Vec<std::net::IpAddr>> {
    spec.as_array()
        .unwrap()
        .iter()
        .map(|list| list.as_array().unwrap().iter().map(|ip| ip.as_str().unwrap().parse().unwrap()).collect())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn unfurls_like_the_reference() {
    let spec: Value = serde_json::from_str(include_str!("../testdata/opengraph_cases.json")).unwrap();
    let expected: Value = serde_json::from_str(include_str!("../testdata/opengraph_expected.json")).unwrap();
    let server = FakeServer::start(spec["routes"].as_array().unwrap().iter().map(route).collect()).await;
    let public: HashSet<std::net::IpAddr> =
        spec["public_ips"].as_array().unwrap().iter().map(|ip| ip.as_str().unwrap().parse().unwrap()).collect();

    let mut failures = Vec::new();
    for (case, expected) in spec["cases"].as_array().unwrap().iter().zip(expected.as_array().unwrap()) {
        let name = case["name"].as_str().unwrap();
        assert_eq!(name, expected["name"].as_str().unwrap());
        let resolver = Arc::new(FakeResolver::default());
        for (host, list) in spec["hosts"].as_object().unwrap() {
            resolver.set(host, answers(list));
        }
        let dialer = Arc::new(MappingDialer { public: public.clone(), to: server.addr, dialed: Mutex::new(Vec::new()) });
        let net = network(resolver.clone(), dialer);
        let before = server.received().len();

        let response = match unfurl(&net, case["url"].as_str().unwrap()).await {
            Ok(Unfurl::Json(body)) => serde_json::json!({ "status": 200, "body": body }),
            Ok(Unfurl::NoContent) => serde_json::json!({ "status": 204 }),
            Err(UnfurlError::Raised(class)) => serde_json::json!({ "status": 500, "error": class }),
        };
        let requests: Vec<Value> = server.received()[before..]
            .iter()
            .map(|r| {
                serde_json::json!([
                    r.method,
                    r.header("host"),
                    r.target,
                    r.header("accept"),
                    r.header("accept-encoding"),
                    r.header("user-agent")
                ])
            })
            .collect();
        let actual = serde_json::json!({ "response": response, "lookups": resolver.lookups(), "requests": requests });
        let wanted =
            serde_json::json!({ "response": expected["response"], "lookups": expected["lookups"], "requests": expected["requests"] });
        if actual != wanted {
            failures.push(format!("{name}:\n  expected {wanted}\n  actual   {actual}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        spec["cases"].as_array().unwrap().len(),
        failures.join("\n")
    );
}

/// test/controllers/unfurl_links_controller_test.rb over plain HTTPS: the pinned address,
/// with the certificate verified against the host name.
#[tokio::test]
async fn unfurls_over_https() {
    let page = "<html><head><meta property=\"og:url\" content=\"https://example.com\"><meta property=\"og:title\" content=\"Hey!\"><meta property=\"og:description\" content=\"desc..\"><meta property=\"og:image\" content=\"https://example.com/image.png\"></head></html>";
    let server = FakeServer::start_tls(vec![
        Route::new("GET", "www.example.com", "/", 200).header("Content-Type", "text/html").body(page),
        Route::new("HEAD", "example.com", "/image.png", 200).header("Content-Type", "image/png"),
    ])
    .await;
    let resolver = Arc::new(FakeResolver::new([("www.example.com", vec!["93.184.216.34"]), ("example.com", vec!["93.184.216.35"])]));
    let public = HashSet::from(["93.184.216.34".parse().unwrap(), "93.184.216.35".parse().unwrap()]);
    let dialer = Arc::new(MappingDialer { public, to: server.addr, dialed: Mutex::new(Vec::new()) });
    let net = network(resolver, dialer.clone());

    let Unfurl::Json(body) = unfurl(&net, "https://www.example.com").await.unwrap() else { panic!("no content") };
    assert_eq!(
        body,
        r#"{"title":"Hey!","url":"https://example.com","image":"https://example.com/image.png","description":"desc..","context_for_validation":{"context":null},"errors":{}}"#
    );
    let dialed: Vec<String> = dialer.dialed.lock().unwrap().iter().map(|a| a.to_string()).collect();
    assert_eq!(dialed, ["93.184.216.34:443", "93.184.216.35:443"]);

    // A certificate that doesn't verify is a failed fetch.
    let untrusted = Network { tls: crate::integrations::net::tls_config(rustls::RootCertStore::empty()), ..net };
    assert_eq!(unfurl(&untrusted, "https://www.example.com").await, Ok(Unfurl::NoContent));
}

/// www.example.com, at a fake public address that connects to `server`.
fn network_to(server: std::net::SocketAddr) -> Network {
    let resolver = Arc::new(FakeResolver::new([("www.example.com", vec!["93.184.216.34"])]));
    let dialer =
        Arc::new(MappingDialer { public: HashSet::from(["93.184.216.34".parse().unwrap()]), to: server, dialed: Mutex::new(Vec::new()) });
    network(resolver, dialer)
}

/// A page followed by a gigabyte of zeros, gzipped to a megabyte, is past the 5MB limit as soon
/// as that much is inflated. The same page followed by less unfurls.
#[tokio::test]
async fn stops_reading_a_gzip_bomb_at_the_limit() {
    use std::io::Write;
    let page = "<meta property=\"og:title\" content=\"Hey!\"><meta property=\"og:url\" content=\"http://www.example.com/\"><meta property=\"og:description\" content=\"desc..\">";
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(page.as_bytes()).unwrap();
    let page = encoder.finish().unwrap();
    let gzipped = |path: &str, zeros: Vec<u8>| {
        Route::new("GET", "*", path, 200)
            .header("Content-Type", "text/html")
            .header("Content-Encoding", "gzip")
            .body([page.clone(), zeros].concat())
    };
    let server = FakeServer::start(vec![gzipped("/", gzip_bomb(1024)), gzipped("/small", gzip_bomb(2))]).await;
    let net = network_to(server.addr);

    assert!(matches!(unfurl(&net, "http://www.example.com/small").await, Ok(Unfurl::Json(_))));
    let started = std::time::Instant::now();
    assert_eq!(unfurl(&net, "http://www.example.com/").await, Ok(Unfurl::NoContent));
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
}

/// A server that keeps sending a byte at a time never trips a read timeout, but the unfurl as a
/// whole gives up.
#[tokio::test]
async fn gives_up_on_a_trickling_page() {
    let server = trickling_server("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n").await;
    let started = std::time::Instant::now();
    let deadline = std::time::Duration::from_millis(500);
    assert_eq!(unfurl_within(&network_to(server), "http://www.example.com/", deadline).await, Ok(Unfurl::NoContent));
    assert!(started.elapsed() < deadline * 2, "{:?}", started.elapsed());
}
