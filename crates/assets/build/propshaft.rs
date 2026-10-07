//! A port of Propshaft 1.2.1's load path, digesting and compilers (the gem's lib/propshaft/*),
//! run at build time so the digested paths and compiled bytes match `assets:precompile`.

use fancy_regex::{Captures, Regex};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Asset {
    pub logical_path: String,
    pub source: PathBuf,
    pub content: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Css,
    Js,
    Other,
}

pub struct LoadPath {
    pub assets: Vec<Asset>,
    by_logical_path: HashMap<String, usize>,
    version: String,
    prefix: String,
    css_asset_urls: Regex,
    js_asset_urls: Regex,
    source_mapping_urls: Regex,
    url_prefix_in_source_map: Regex,
    already_digested: Regex,
}

// Ruby's \s is [ \t\n\v\f\r]; spelled out so the Latin-1 decoded input can't match more.
const WS: &str = r"[ \t\n\x0B\x0C\r]";

impl LoadPath {
    /// Propshaft::LoadPath#assets_by_path: earlier paths win for the same logical path, and
    /// dotfiles are skipped (but not dot-directories).
    pub fn new(paths: &[PathBuf], version: &str, prefix: &str, include: impl Fn(&Path) -> bool) -> Self {
        let mut assets = Vec::new();
        let mut by_logical_path = HashMap::new();

        for path in dedup(paths) {
            if !path.exists() {
                continue;
            }
            let mut files = Vec::new();
            all_files_from_tree(&path, &mut files, &include);
            files.sort();
            for file in files {
                if file.file_name().unwrap().to_string_lossy().starts_with('.') {
                    continue;
                }
                let logical_path = file.strip_prefix(&path).unwrap().to_string_lossy().into_owned();
                if by_logical_path.contains_key(&logical_path) {
                    continue;
                }
                let content = fs::read(&file).unwrap_or_else(|e| panic!("reading {}: {e}", file.display()));
                by_logical_path.insert(logical_path.clone(), assets.len());
                assets.push(Asset { logical_path, source: file, content });
            }
        }

        let quoted_url = |head: &str, excluded: &str| {
            format!(r#"{head}\({WS}*["']?(?!(?:{excluded}))([^"' \t\n\x0B\x0C\r?#)]+)([#?][^"')]+)?{WS}*["']?\)"#)
        };

        LoadPath {
            assets,
            by_logical_path,
            version: version.to_string(),
            prefix: prefix.to_string(),
            // Propshaft::Compiler::CssAssetUrls::ASSET_URL_PATTERN
            css_asset_urls: Regex::new(&quoted_url("url", r"\#|%23|data:|http:|https:|//")).unwrap(),
            // Propshaft::Compiler::JsAssetUrls::ASSET_URL_PATTERN
            js_asset_urls: Regex::new(&quoted_url("RAILS_ASSET_URL", r"\#|%23|data|http|//")).unwrap(),
            // Propshaft::Compiler::SourceMappingUrls::SOURCE_MAPPING_PATTERN, with Ruby's \Z
            source_mapping_urls: Regex::new(&format!(r"(//|/\*)# sourceMappingURL=(.+\.map)({WS}*?\*/)?{WS}*?(?=\n?\z)")).unwrap(),
            url_prefix_in_source_map: Regex::new(&format!(r"(?m)^(.+/)?{}/", fancy_regex::escape(prefix))).unwrap(),
            already_digested: Regex::new(r"-([0-9a-zA-Z_-]{7,128})\.digested").unwrap(),
        }
    }

    pub fn find(&self, logical_path: &str) -> Option<usize> {
        self.by_logical_path.get(logical_path).copied()
    }

    /// Propshaft::Asset#content_type, reduced to the two types that have compilers.
    pub fn kind(&self, index: usize) -> Kind {
        match extname(&self.assets[index].logical_path).strip_prefix('.') {
            Some("css") => Kind::Css,
            Some("js") => Kind::Js,
            _ => Kind::Other,
        }
    }

    /// Propshaft::Asset#digest: SHA1 of the content, the content of everything it references
    /// (recursively, in discovery order) and the assets version.
    pub fn digest(&self, index: usize) -> String {
        let mut hasher = Sha1::new();
        hasher.update(&self.assets[index].content);
        for referenced in self.referenced_by(index) {
            hasher.update(&self.assets[referenced].content);
        }
        hasher.update(self.version.as_bytes());
        hex(&hasher.finalize())[..8].to_string()
    }

    /// Propshaft::Asset#digested_path
    pub fn digested_path(&self, index: usize) -> String {
        let logical_path = &self.assets[index].logical_path;
        if self.already_digested.is_match(logical_path).unwrap() {
            return logical_path.clone();
        }
        match digestable_extension_start(logical_path) {
            Some(dot) => format!("{}-{}{}", &logical_path[..dot], self.digest(index), &logical_path[dot..]),
            None => logical_path.clone(),
        }
    }

    /// Propshaft::Compilers#compile: None when no compiler is registered for the type.
    pub fn compiled_content(&self, index: usize) -> Option<Vec<u8>> {
        let asset_urls = match self.kind(index) {
            Kind::Css => &self.css_asset_urls,
            Kind::Js => &self.js_asset_urls,
            Kind::Other => return None,
        };
        let input = latin1_decode(&self.assets[index].content);
        let input = self.compile_asset_urls(index, asset_urls, &input);
        let output = self.compile_source_mapping_urls(index, &input);
        Some(latin1_encode(&output))
    }

    fn referenced_by(&self, index: usize) -> Vec<usize> {
        let pattern = match self.kind(index) {
            Kind::Css => &self.css_asset_urls,
            Kind::Js => &self.js_asset_urls,
            Kind::Other => return Vec::new(),
        };
        let mut references = Vec::new();
        self.collect_references(index, pattern, &mut references);
        references
    }

    // CssAssetUrls#referenced_by / JsAssetUrls#referenced_by: referenced assets are scanned with
    // the same pattern whatever their own type is.
    fn collect_references(&self, index: usize, pattern: &Regex, references: &mut Vec<usize>) {
        let content = latin1_decode(&self.assets[index].content);
        let directory = dirname(&self.assets[index].logical_path);
        for captures in pattern.captures_iter(&content) {
            let captures = captures.unwrap();
            let url = latin1_to_utf8(&captures[1]);
            if let Some(referenced) = self.find(&resolve_path(&directory, &url))
                && !references.contains(&referenced)
            {
                references.push(referenced);
                self.collect_references(referenced, pattern, references);
            }
        }
    }

    fn url_prefix(&self) -> &str {
        self.prefix.trim_end_matches('/')
    }

    fn compile_asset_urls(&self, index: usize, pattern: &Regex, input: &str) -> String {
        let directory = dirname(&self.assets[index].logical_path);
        let is_css = self.kind(index) == Kind::Css;
        gsub(pattern, input, |captures| {
            let url = latin1_to_utf8(&captures[1]);
            let fingerprint = captures.get(2).map(|m| m.as_str()).unwrap_or("");
            let replacement = match self.find(&resolve_path(&directory, &url)) {
                Some(found) => format!("\"{}/{}{}\"", self.url_prefix(), self.digested_path(found), latin1_to_utf8(fingerprint)),
                None => format!("\"{url}\""),
            };
            let replacement = if is_css { format!("url({replacement})") } else { replacement };
            utf8_to_latin1(&replacement)
        })
    }

    fn compile_source_mapping_urls(&self, index: usize, input: &str) -> String {
        let logical_path = &self.assets[index].logical_path;
        gsub(&self.source_mapping_urls, input, |captures| {
            let comment_start = &captures[1];
            let comment_end = captures.get(3).map(|m| m.as_str()).unwrap_or("");
            let url = latin1_to_utf8(&captures[2]);
            let url = self.url_prefix_in_source_map.replace_all(&url, "").into_owned();
            let directory = dirname(logical_path);
            let resolved = if directory == "." { url } else { plus(&directory, &url) };
            match self.find(&resolved) {
                Some(found) => format!(
                    "{comment_start}# sourceMappingURL={}/{}{comment_end}",
                    self.url_prefix(),
                    utf8_to_latin1(&self.digested_path(found))
                ),
                None => format!("{comment_start}{comment_end}"),
            }
        })
    }
}

fn gsub(pattern: &Regex, input: &str, mut replace: impl FnMut(&Captures) -> String) -> String {
    let mut output = String::with_capacity(input.len());
    let mut last = 0;
    for captures in pattern.captures_iter(input) {
        let captures = captures.unwrap();
        let whole = captures.get(0).unwrap();
        output.push_str(&input[last..whole.start()]);
        output.push_str(&replace(&captures));
        last = whole.end();
    }
    output.push_str(&input[last..]);
    output
}

/// Propshaft::LoadPath#dedup: drop paths nested in (string-prefixed by) an earlier sorted path,
/// keeping the original order. Pathname sorts with "/" below every other character.
fn dedup(paths: &[PathBuf]) -> Vec<PathBuf> {
    let key = |p: &PathBuf| p.to_string_lossy().replace('/', "\0");
    let mut sorted: Vec<&PathBuf> = paths.iter().collect();
    sorted.sort_by_key(|p| key(p));
    let mut deduped: Vec<String> = Vec::new();
    for path in sorted {
        let path = path.to_string_lossy().into_owned();
        if deduped.last().is_none_or(|last| !path.starts_with(last.as_str())) {
            deduped.push(path);
        }
    }
    let mut seen = Vec::new();
    paths
        .iter()
        .filter(|p| {
            let s = p.to_string_lossy().into_owned();
            if deduped.contains(&s) && !seen.contains(&s) {
                seen.push(s);
                true
            } else {
                false
            }
        })
        .cloned()
        .collect()
}

fn all_files_from_tree(path: &Path, files: &mut Vec<PathBuf>, include: &impl Fn(&Path) -> bool) {
    for entry in fs::read_dir(path).unwrap() {
        let child = entry.unwrap().path();
        if !include(&child) {
            continue;
        }
        if child.is_dir() {
            all_files_from_tree(&child, files, include);
        } else {
            files.push(child);
        }
    }
}

/// The start of the extension that `sub(/\.(\w+(\.map)?)$/)` replaces in Asset#digested_path.
fn digestable_extension_start(path: &str) -> Option<usize> {
    let is_word = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
    path.char_indices().filter(|&(_, c)| c == '.').map(|(i, _)| i).find(|&dot| {
        let rest = &path[dot + 1..];
        is_word(rest) || rest.strip_suffix(".map").is_some_and(is_word)
    })
}

/// Ruby's File.extname.
pub fn extname(path: &str) -> &str {
    let base = path.rsplit('/').next().unwrap();
    let trimmed = base.trim_start_matches('.');
    match trimmed.rfind('.') {
        Some(dot) => &trimmed[dot..],
        None => "",
    }
}

/// Pathname#dirname for relative logical paths.
fn dirname(path: &str) -> String {
    match path.rfind('/') {
        Some(slash) => path[..slash].to_string(),
        None => ".".to_string(),
    }
}

/// CssAssetUrls#resolve_path (JsAssetUrls has the same one).
fn resolve_path(directory: &str, filename: &str) -> String {
    if filename.starts_with("../") {
        cleanpath(&plus(directory, filename))
    } else if let Some(absolute) = filename.strip_prefix('/') {
        absolute.to_string()
    } else {
        plus(directory, filename.strip_prefix("./").unwrap_or(filename))
    }
}

/// Pathname#+ for relative paths: leading "." and ".." of the right side are resolved against
/// the left side; anything after the first ordinary component is kept as written.
fn plus(left: &str, right: &str) -> String {
    let mut prefix: Vec<&str> = left.split('/').filter(|c| !c.is_empty()).collect();
    let mut suffix: Vec<&str> = right.split('/').filter(|c| !c.is_empty()).collect();
    let mut kept = Vec::new();
    loop {
        while suffix.first() == Some(&".") {
            suffix.remove(0);
        }
        let Some(last) = prefix.pop() else { break };
        if last == "." {
            continue;
        }
        if last == ".." || suffix.first() != Some(&"..") {
            kept.push(last);
            break;
        }
        suffix.remove(0);
    }
    prefix.extend(kept);
    let joined: Vec<&str> = prefix.into_iter().chain(suffix).collect();
    if joined.is_empty() { ".".to_string() } else { joined.join("/") }
}

/// Pathname#cleanpath (non-conservative) for relative paths.
fn cleanpath(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." if out.last().is_some_and(|c| *c != "..") => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    if out.is_empty() { ".".to_string() } else { out.join("/") }
}

// Propshaft reads assets as ASCII-8BIT and matches bytes; decoding each byte to the char with the
// same code point lets a str regex engine see exactly those bytes.
fn latin1_decode(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn latin1_encode(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

fn latin1_to_utf8(s: &str) -> String {
    String::from_utf8_lossy(&latin1_encode(s)).into_owned()
}

fn utf8_to_latin1(s: &str) -> String {
    latin1_decode(s.as_bytes())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
