//! Digests and compiles the reference's assets the way `bin/rails assets:precompile` does
//! (Propshaft), renders the import map, and embeds the results plus reference/public into the
//! crate as `$OUT_DIR/embedded.rs`.

#[path = "build/importmap.rs"]
mod importmap;
#[path = "build/propshaft.rs"]
mod propshaft;

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ruby_compat::erb::html_escape;

// Propshaft's Railtie default; the app doesn't change it.
const PREFIX: &str = "/assets";

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let rails_root = crate_dir.join("../../reference").canonicalize().expect("reference/ submodule is missing");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    for watched in ["app/assets", "app/javascript", "vendor/javascript", "public", "config/importmap.rb", "config/initializers/assets.rb"] {
        println!("cargo:rerun-if-changed={}", rails_root.join(watched).display());
    }
    println!("cargo:rerun-if-changed={}", crate_dir.join("vendor").display());
    let overrides = crate_dir.join("overrides");
    let include = |path: &Path| {
        !path.starts_with(overrides.join("public"))
            && match path.strip_prefix(overrides.join("basecoat")) {
                Ok(relative) => matches!(relative.to_str(), Some("" | "app.css" | "app.js" | "theme-init.js")),
                Err(_) => true,
            }
            && match path.strip_prefix(overrides.join("lucide")) {
                Ok(relative) => matches!(relative.to_str(), Some("" | "catalog.json" | "LICENSE.txt")),
                Err(_) => true,
            }
    };
    println!("cargo:rerun-if-changed={}", overrides.display());
    println!("cargo:rerun-if-changed=build");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    let load_path = propshaft::LoadPath::new(&load_path_dirs(&crate_dir, &rails_root), &assets_version(&rails_root), PREFIX, include);
    let compiled_dir = out_dir.join("compiled");
    let _ = fs::remove_dir_all(&compiled_dir);

    let mut code = String::new();
    let mut entries = Vec::new(); // (logical, digested, body ident)
    for (index, asset) in load_path.assets.iter().enumerate() {
        let digested = load_path.digested_path(index);
        let body_path = match load_path.compiled_content(index) {
            Some(compiled) => {
                let path = compiled_dir.join(&digested);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, compiled).unwrap();
                path
            }
            None => asset.source.clone(),
        };
        writeln!(code, "static ASSET_{index}: &[u8] = include_bytes!({:?});", body_path.display().to_string()).unwrap();
        entries.push((asset.logical_path.clone(), digested, format!("ASSET_{index}")));
    }

    // Propshaft::Processor#write_manifest (the order is the load path's, not readdir's).
    let manifest_json = format!(
        "{{{}}}",
        entries
            .iter()
            .map(|(logical, digested, _)| format!("{}:{{\"digested_path\":{},\"integrity\":null}}", json(logical), json(digested)))
            .collect::<Vec<_>>()
            .join(",")
    );

    let mut by_logical: Vec<&(String, String, String)> = entries.iter().collect();
    by_logical.sort_by(|a, b| a.0.cmp(&b.0));
    code.push_str("/// (logical path, digested path), sorted by logical path.\n");
    code.push_str("pub(crate) static MANIFEST: &[(&str, &str)] = &[\n");
    for (logical, digested, _) in &by_logical {
        writeln!(code, "    ({logical:?}, {digested:?}),").unwrap();
    }
    code.push_str("];\n");

    // Propshaft::Helper#all_stylesheets_paths: every text/css asset's logical path, sorted.
    code.push_str("pub(crate) static STYLESHEETS: &[&str] = &[\n");
    for (index, _) in by_logical.iter().enumerate().filter(|(_, e)| propshaft::extname(&e.0) == ".css") {
        writeln!(code, "    {:?},", by_logical[index].0).unwrap();
    }
    code.push_str("];\n");

    // Everything ActionDispatch::Static can serve: reference/public plus the precompiled
    // public/assets (the digested files and the manifest), sorted by URL path.
    let mut files: Vec<(String, String)> = Vec::new();
    let public = rails_root.join("public");
    let mut public_files = Vec::new();
    all_files(&public, &mut public_files);
    // A stray precompile inside the submodule mustn't shadow what we build.
    public_files.retain(|file| !file.starts_with(public.join("assets")));
    for (i, file) in public_files.iter().enumerate() {
        let url = format!("/{}", file.strip_prefix(&public).unwrap().display());
        let override_path = overrides.join("public").join(file.strip_prefix(&public).unwrap());
        let file = if override_path.is_file() { &override_path } else { file };
        writeln!(code, "static PUBLIC_{i}: &[u8] = include_bytes!({:?});", file.display().to_string()).unwrap();
        files.push((url, format!("PUBLIC_{i}")));
    }
    for (_, digested, body) in &entries {
        files.push((format!("{PREFIX}/{digested}"), body.clone()));
    }
    files.push((format!("{PREFIX}/.manifest.json"), "MANIFEST_JSON.as_bytes()".to_string()));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);
    code.push_str("pub(crate) static FILES: &[(&str, &[u8])] = &[\n");
    for (url, body) in &files {
        writeln!(code, "    ({url:?}, {body}),").unwrap();
    }
    code.push_str("];\n");

    writeln!(code, "pub(crate) static MANIFEST_JSON: &str = {manifest_json:?};").unwrap();
    writeln!(code, "pub(crate) static IMPORTMAP_TAGS: &str = {:?};", importmap_tags(&load_path, &entries, &rails_root)).unwrap();
    writeln!(code, "pub(crate) static BUILT_AT: &str = {:?};", httpdate(build_time())).unwrap();

    fs::write(out_dir.join("embedded.rs"), code).unwrap();
    embed_lucide(&overrides, &out_dir);
}

fn embed_lucide(overrides: &Path, out_dir: &Path) {
    let catalog = fs::read_to_string(overrides.join("lucide/catalog.json")).expect("Lucide catalog is missing; regenerate frontend assets");
    let icons: serde_json::Value = serde_json::from_str(&catalog).expect("invalid Lucide catalog");
    let mut output = String::from("static LUCIDE_ICONS: &[LucideIcon] = &[\n");
    let mut previous = "";
    for icon in icons.as_array().expect("Lucide catalog must be an array") {
        let name = icon["name"].as_str().expect("Lucide icon name");
        let label = icon["label"].as_str().expect("Lucide icon label");
        let svg = icon["svg"].as_str().expect("Lucide icon SVG");
        assert!(name > previous, "Lucide catalog must contain unique, sorted names");
        previous = name;
        writeln!(output, "    LucideIcon {{ name: {name:?}, label: {label:?}, svg: {svg:?} }},").unwrap();
    }
    output.push_str("];\n");
    fs::write(out_dir.join("lucide.rs"), output).unwrap();
}

/// `overrides/` first, so the app's own changes to the frontend shadow the reference's files of
/// the same logical path, then vendor/LOAD_PATH (written by script/revendor from
/// `Rails.application.assets.load_path.paths`).
fn load_path_dirs(crate_dir: &Path, rails_root: &Path) -> Vec<PathBuf> {
    let overrides = crate_dir.join("overrides");
    let load_path =
        fs::read_to_string(crate_dir.join("vendor/LOAD_PATH")).expect("vendor/LOAD_PATH is missing; run crates/assets/script/revendor");
    let exported = load_path.lines().filter(|line| !line.trim().is_empty()).map(|line| match line.split_once(':') {
        Some(("reference", dir)) => rails_root.join(dir),
        Some(("vendor", dir)) => crate_dir.join("vendor").join(dir),
        _ => panic!("vendor/LOAD_PATH: bad line {line:?}"),
    });
    std::iter::once(overrides).chain(exported).collect()
}

/// config/initializers/assets.rb sets `Rails.application.config.assets.version`.
fn assets_version(rails_root: &Path) -> String {
    let initializer = fs::read_to_string(rails_root.join("config/initializers/assets.rb")).unwrap_or_default();
    initializer
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .find_map(|line| {
            let (_, value) = line.split_once("config.assets.version")?;
            let value = value.trim_start().strip_prefix('=')?.trim();
            Some(value.trim_matches(|c| c == '"' || c == '\'').to_string())
        })
        .unwrap_or_else(|| "1".to_string())
}

/// Importmap::ImportmapTagsHelper#javascript_importmap_tags for the "application" entry point,
/// with no CSP nonce (the reference configures no content security policy).
fn importmap_tags(load_path: &propshaft::LoadPath, entries: &[(String, String, String)], rails_root: &Path) -> String {
    let resolve = |path: &str| load_path.find(path).map(|index| format!("{PREFIX}/{}", entries[index].1));
    let mut pins = importmap::expand(&rails_root.join("config/importmap.rb"), rails_root);
    // Matchbox controllers have no counterpart in reference/app/javascript, but must still
    // participate in Stimulus discovery and the same digested importmap as inherited ones.
    let mut controllers: Vec<_> =
        entries.iter().filter(|(path, _, _)| path.starts_with("controllers/") && path.ends_with("_controller.js")).collect();
    controllers.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, _, _) in controllers {
        let name = path.strip_suffix(".js").unwrap();
        if !pins.iter().any(|pin| pin.name == name) {
            pins.push(importmap::Pin { name: name.to_string(), path: path.clone(), preload: true });
        }
    }

    // Missing assets are skipped (Propshaft::MissingAssetError is a rescuable asset error).
    let imports: Vec<(String, String)> = pins.iter().filter_map(|pin| Some((pin.name.clone(), resolve(&pin.path)?))).collect();
    let json = if imports.is_empty() {
        "{\n  \"imports\": {}\n}".to_string()
    } else {
        let lines: Vec<String> = imports.iter().map(|(name, path)| format!("    {}: {}", json(name), json(path))).collect();
        format!("{{\n  \"imports\": {{\n{}\n  }}\n}}", lines.join(",\n"))
    };

    let mut preloads: Vec<String> = Vec::new();
    for pin in pins.iter().filter(|pin| pin.preload) {
        if let Some(path) = resolve(&pin.path)
            && !preloads.contains(&path)
        {
            preloads.push(path);
        }
    }

    let mut tags = vec![format!("<script type=\"importmap\" data-turbo-track=\"reload\">{json}</script>")];
    tags.push(
        preloads.iter().map(|path| format!("<link rel=\"modulepreload\" href=\"{}\">", html_escape(path))).collect::<Vec<_>>().join("\n"),
    );
    tags.push("<script type=\"module\">import \"application\"</script>".to_string());
    tags.join("\n")
}

fn all_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            all_files(&path, files);
        } else {
            files.push(path);
        }
    }
    files.sort();
}

/// JSON.generate's string escaping (no script_safe, non-ASCII passed through).
fn json(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn build_time() -> u64 {
    env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs())
}

/// Time#httpdate: "Sat, 26 Sep 2026 12:23:14 GMT".
fn httpdate(epoch: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let days = (epoch / 86_400) as i64;
    let secs = epoch % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        DAYS[(days.rem_euclid(7)) as usize],
        day,
        MONTHS[(month - 1) as usize],
        year,
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}
