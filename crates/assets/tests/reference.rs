//! Golden tests against what the reference app produced (tests/reference/*, written by
//! script/revendor from `assets:precompile` and the real Rails helpers). Files in `overrides/`
//! deliberately differ from the reference, so only their digests and bytes are allowed to.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/reference/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn json_fixture(name: &str) -> Value {
    serde_json::from_str(&fixture(name)).unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The logical paths in `overrides/`.
fn override_files() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("overrides");
    let mut files = Vec::new();
    collect_files(&dir, &mut files);
    files
        .into_iter()
        .map(|file| file.strip_prefix(&dir).unwrap().to_string_lossy().into_owned())
        .filter(|logical| {
            !logical.starts_with("public/")
                && (!logical.starts_with("basecoat/")
                    || matches!(logical.as_str(), "basecoat/app.css" | "basecoat/app.js" | "basecoat/theme-init.js"))
                && (!logical.starts_with("lucide/") || matches!(logical.as_str(), "lucide/catalog.json" | "lucide/LICENSE.txt"))
        })
        .collect()
}

/// Each overridden logical path, with the reference's digested path and ours.
fn overridden() -> BTreeMap<String, (String, String)> {
    let reference = json_fixture("manifest.json");
    let ours: BTreeMap<&str, &str> = matchbox_assets::manifest().iter().map(|(l, d)| (*l, *d)).collect();
    override_files()
        .into_iter()
        .filter_map(|logical| {
            let theirs = reference[&logical]["digested_path"].as_str()?;
            let ours = ours[logical.as_str()];
            assert_ne!(theirs, ours, "{logical} is overridden but digests the same");
            Some((logical, (theirs.to_string(), ours.to_string())))
        })
        .collect()
}

/// Logical paths the overrides add, which the reference doesn't have at all.
fn added() -> Vec<String> {
    let reference = json_fixture("manifest.json");
    override_files().into_iter().filter(|logical| reference.get(logical).is_none()).collect()
}

fn collect_files(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name == "node_modules") {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, files);
        } else {
            files.push(path);
        }
    }
}

/// `text` with our digested paths for overridden files replaced by the reference's.
fn as_reference(text: &str) -> String {
    overridden().values().fold(text.to_string(), |text, (theirs, ours)| text.replace(ours.as_str(), theirs))
}

fn get(path: &str) -> matchbox_assets::StaticResponse {
    matchbox_assets::serve(&matchbox_assets::StaticRequest { method: "GET", path, ..Default::default() })
        .unwrap_or_else(|| panic!("{path} isn't served"))
}

#[test]
fn manifest_matches_the_reference_precompile() {
    let reference: BTreeMap<String, String> = json_fixture("manifest.json")
        .as_object()
        .unwrap()
        .iter()
        .map(|(logical, entry)| (logical.clone(), entry["digested_path"].as_str().unwrap().to_string()))
        .collect();
    let added = added();
    let ours: BTreeMap<String, String> = matchbox_assets::manifest()
        .iter()
        .filter(|(l, _)| !added.iter().any(|a| a == l))
        .map(|(l, d)| (l.to_string(), as_reference(d)))
        .collect();

    let missing: Vec<_> = reference.iter().filter(|(l, d)| ours.get(*l) != Some(d)).collect();
    let extra: Vec<_> = ours.keys().filter(|l| !reference.contains_key(*l)).collect();
    assert!(missing.is_empty() && extra.is_empty(), "differs from reference: {missing:?}, extra: {extra:?}");

    let served: Value = serde_json::from_str(&as_reference(matchbox_assets::manifest_json())).unwrap();
    let mut served = served.as_object().unwrap().clone();
    for logical in &added {
        served.remove(logical);
    }
    let mut reference_json = json_fixture("manifest.json").as_object().unwrap().clone();
    // An overridden file's integrity hash covers its own bytes.
    for logical in overridden().keys() {
        served[logical].as_object_mut().unwrap().remove("integrity");
        reference_json[logical].as_object_mut().unwrap().remove("integrity");
    }
    assert_eq!(served, reference_json);
}

#[test]
fn compiled_files_are_byte_identical_to_the_reference_precompile() {
    let reference = json_fixture("compiled_sha256.json");
    let overridden: Vec<String> = overridden().into_values().map(|(theirs, _)| theirs).collect();
    let mut mismatched = Vec::new();
    for (digested_path, expected) in reference.as_object().unwrap() {
        if overridden.contains(digested_path) {
            continue;
        }
        let response = get(&format!("/assets/{digested_path}"));
        if sha256(&response.body) != expected.as_str().unwrap() {
            mismatched.push(digested_path.clone());
        }
    }
    assert!(mismatched.is_empty(), "compiled output differs for {mismatched:?}");
    assert_eq!(reference.as_object().unwrap().len() + added().len(), matchbox_assets::manifest().len());
}

#[test]
fn workspace_styles_follow_the_reference_stylesheets_and_preloads() {
    let tags = matchbox_assets::stylesheet_link_tag_all(&[("data-turbo-track", "reload")]);
    let workspace = matchbox_assets::stylesheet_link_tag(&["zz-matchbox.css"], &[("data-turbo-track", "reload")]);
    assert_eq!(tags.html, format!("{}\n{}", fixture("stylesheet_link_tag_all.html"), workspace.html));
    assert_eq!(tags.preload_links.last(), workspace.preload_links.first());
    // The existing Rails-sized header budget is already full before the final override.
    assert_eq!(matchbox_assets::append_preload_links("", &tags.preload_links), fixture("link_header.txt"));
}

#[test]
fn basecoat_build_inputs_are_not_published() {
    let published: Vec<_> =
        matchbox_assets::manifest().iter().map(|(logical, _)| *logical).filter(|p| p.starts_with("basecoat/")).collect();
    assert_eq!(published, ["basecoat/app.css", "basecoat/app.js", "basecoat/theme-init.js"]);
    assert!(matchbox_assets::try_asset_path("basecoat/src/theme.css").is_err());
}

#[test]
fn lucide_catalog_matches_the_embedded_icons() {
    let published: Vec<_> = matchbox_assets::manifest().iter().map(|(logical, _)| *logical).filter(|p| p.starts_with("lucide/")).collect();
    assert_eq!(published, ["lucide/LICENSE.txt", "lucide/catalog.json"]);
    let catalog = get(&matchbox_assets::asset_path("lucide/catalog.json"));
    assert_eq!(catalog.header("content-type"), Some("application/json"));
    let catalog: Value = serde_json::from_slice(&catalog.body).unwrap();
    let icons = matchbox_assets::lucide_icons();
    let catalog = catalog.as_array().unwrap();
    assert_eq!(catalog.len(), icons.len());
    for (entry, icon) in catalog.iter().zip(icons) {
        assert_eq!(entry["name"], icon.name);
        assert_eq!(entry["label"], icon.label);
        assert_eq!(entry["svg"], icon.svg);
    }
}

#[test]
fn style_profiles_keep_stylesheets_and_preloads_separate() {
    use matchbox_assets::{StyleProfile, stylesheet_link_tag_for};
    let legacy = stylesheet_link_tag_for(StyleProfile::Legacy, &[("data-turbo-track", "reload")]);
    assert!(legacy.html.starts_with(&fixture("stylesheet_link_tag_all.html")));
    assert_eq!(matchbox_assets::stylesheet_paths_for(StyleProfile::Legacy).last(), Some(&"zz-matchbox.css"));
    assert!(legacy.preload_links.iter().all(|link| !link.contains("basecoat/")));
    let basecoat = stylesheet_link_tag_for(StyleProfile::Basecoat, &[("data-turbo-track", "reload")]);
    assert_eq!(basecoat.preload_links.len(), 1);
    assert!(basecoat.preload_links[0].contains(&matchbox_assets::asset_path("basecoat/app.css")));
    assert_eq!(basecoat.html.matches("<link").count(), 1);
    assert!(basecoat.html.contains("data-turbo-track=\"reload\""));
    assert!(!basecoat.html.contains("zz-matchbox"));
}

#[test]
fn javascript_importmap_tags_match_the_reference() {
    let ours = as_reference(matchbox_assets::javascript_importmap_tags());
    let reference = fixture("javascript_importmap_tags.html");
    let imports = |tags: &str| {
        let json = tags.split_once('>').unwrap().1.split_once("</script>").unwrap().0;
        serde_json::from_str::<Value>(json).unwrap()
    };
    let mut our_imports = imports(&ours);
    let mut inherited_tags = ours.clone();
    for path in added().iter().filter(|path| path.starts_with("controllers/") && path.ends_with(".js")) {
        let name = path.strip_suffix(".js").unwrap();
        let url = matchbox_assets::asset_path(path);
        assert_eq!(our_imports["imports"][name], url, "owned controller must be discoverable by Stimulus");
        our_imports["imports"].as_object_mut().unwrap().remove(name);
        inherited_tags = inherited_tags.replace(&format!("<link rel=\"modulepreload\" href=\"{url}\">\n"), "");
        inherited_tags = inherited_tags.replace(&format!("\n<link rel=\"modulepreload\" href=\"{url}\">"), "");
    }
    assert_eq!(our_imports, imports(&reference));
    assert_eq!(inherited_tags.split_once("</script>").unwrap().1, reference.split_once("</script>").unwrap().1);
}

#[test]
fn public_files_are_served_like_action_dispatch_static() {
    let overridden = overridden();
    for case in json_fixture("static_responses.json").as_array().unwrap() {
        let env = &case["env"];
        let path = case["path"].as_str().unwrap();
        let override_of = overridden.values().find(|(theirs, _)| path == format!("/assets/{theirs}"));
        let our_path = override_of.map(|(_, ours)| format!("/assets/{ours}"));
        let request = matchbox_assets::StaticRequest {
            method: case["method"].as_str().unwrap(),
            path: our_path.as_deref().unwrap_or(path),
            range: env["HTTP_RANGE"].as_str(),
            accept_encoding: env["HTTP_ACCEPT_ENCODING"].as_str(),
            if_modified_since: None,
        };
        let label = format!("{} {} {env}", request.method, request.path);
        let expected_status = case["status"].as_u64().unwrap();
        let expected_headers = case["headers"].as_object().unwrap();

        // The probe's fallthrough app answers 404 with x-cascade: pass.
        if expected_status == 404 && expected_headers.get("x-cascade").is_some() {
            assert!(matchbox_assets::serve(&request).is_none(), "{label} should fall through");
            continue;
        }

        let response = matchbox_assets::serve(&request).unwrap_or_else(|| panic!("{label} not served"));
        assert_eq!(response.status as u64, expected_status, "{label}");
        if path == "/502.html" {
            if request.method == "GET" {
                assert!(String::from_utf8_lossy(&response.body).contains("Starting Matchbox"));
                assert!(!String::from_utf8_lossy(&response.body).contains("Campfire"));
            }
            continue;
        }

        let ours: BTreeMap<String, String> = response
            .headers
            .iter()
            .filter(|(name, _)| *name != "last-modified")
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect();
        let theirs: BTreeMap<String, String> =
            expected_headers.iter().map(|(name, value)| (name.clone(), value.as_str().unwrap().to_string())).collect();

        // Our manifest lists the same entries in load-path order rather than the build
        // machine's readdir order, so its length and bytes can't match; an overridden file's
        // length, ETag and bytes are its own.
        if request.path == "/assets/.manifest.json" || override_of.is_some() {
            assert_eq!(ours.get("content-type"), theirs.get("content-type"), "{label}");
            continue;
        }

        assert_eq!(ours, theirs, "{label}");
        if request.method == "GET" {
            assert_eq!(sha256(&response.body), case["body_sha256"].as_str().unwrap(), "{label}");
        }
    }
}

#[test]
fn last_modified_round_trips_to_a_304() {
    let response = get("/robots.txt");
    let last_modified = response.header("last-modified").unwrap().to_string();
    let not_modified = matchbox_assets::serve(&matchbox_assets::StaticRequest {
        method: "GET",
        path: "/robots.txt",
        if_modified_since: Some(&last_modified),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(not_modified.status, 304);
    assert!(not_modified.headers.is_empty() && not_modified.body.is_empty());
}

#[test]
fn head_requests_have_no_body() {
    let response =
        matchbox_assets::serve(&matchbox_assets::StaticRequest { method: "HEAD", path: "/robots.txt", ..Default::default() }).unwrap();
    assert_eq!(response.status, 200);
    assert!(response.body.is_empty());
    assert_eq!(response.header("content-length"), Some("99"));
}

#[test]
fn multiple_ranges_are_multipart() {
    let sound = matchbox_assets::audio_path("56k.mp3");
    let response = matchbox_assets::serve(&matchbox_assets::StaticRequest {
        method: "GET",
        path: &sound,
        range: Some("bytes=0-1, 4-5"),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(response.status, 206);
    // Rack sets multipart/byteranges, then Static overwrites it with the file's type.
    assert_eq!(response.header("content-type"), Some("audio/mpeg"));
    let body = String::from_utf8_lossy(&response.body);
    assert!(body.starts_with("\r\n--AaB03x\r\ncontent-type: audio/mpeg\r\ncontent-range: bytes 0-1/"));
    assert!(body.ends_with("\r\n--AaB03x--\r\n"));
}
