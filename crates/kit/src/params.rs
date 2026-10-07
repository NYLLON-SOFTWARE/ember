//! Rails request parameters.
//!
//! This is a port of `ActionDispatch::ParamBuilder` (Rails 8.2, itself derived from
//! `Rack::QueryParser`) plus the pair splitting of `ActionDispatch::QueryParser.each_pair` and
//! `Rack::QueryParser#parse_query_pairs`, the deep munging applied to JSON bodies
//! (`Request::Utils::NoNilParamEncoder`), and the strong-parameters subset Matchbox uses
//! (`require`, `permit`, `fetch`).
//!
//! [`parse_nested`] exposes the query-string parser as JSON for differential tests against
//! `ActionDispatch::ParamBuilder.from_query_string` in the reference container.

use std::path::Path;
use std::sync::Arc;

/// Rails' `ActionDispatch::ParamBuilder.default` depth limit.
pub const DEPTH_LIMIT: usize = 100;
/// `Rack::QueryParser` limits, applied to form bodies (Rack parses those, not Rails).
pub const FORM_BYTESIZE_LIMIT: usize = 4 * 1024 * 1024;
pub const FORM_PARAMS_LIMIT: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParamError {
    /// `ParameterTypeError`: e.g. `a=1&a[b]=2`.
    #[error("{0}")]
    Type(String),
    /// `InvalidParameterError`: bad %-encoding or invalid UTF-8.
    #[error("{0}")]
    Invalid(String),
    /// `ParamsTooDeepError`.
    #[error("exceeded available parameter key space")]
    TooDeep,
    /// `QueryLimitError`.
    #[error("{0}")]
    Limit(String),
    /// `ActionDispatch::Http::Parameters::ParseError` (malformed JSON or multipart).
    #[error("Error occurred while parsing request parameters")]
    Parse,
}

/// A parameter value: what a Rails params hash can hold.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    Str(String),
    File(Arc<UploadedFile>),
    Array(Vec<Param>),
    Hash(ParamMap),
}

impl Param {
    pub fn is_null(&self) -> bool {
        matches!(self, Param::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Param::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_hash(&self) -> Option<&ParamMap> {
        match self {
            Param::Hash(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Param]> {
        match self {
            Param::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_file(&self) -> Option<&Arc<UploadedFile>> {
        match self {
            Param::File(file) => Some(file),
            _ => None,
        }
    }

    /// `params[:a][:b]` style lookup; `None` for anything that isn't a hash.
    pub fn get(&self, key: &str) -> Option<&Param> {
        self.as_hash().and_then(|map| map.get(key))
    }

    /// The value as Rails would interpolate it (`to_s`): strings as-is, numbers and booleans
    /// formatted, nil as "".
    pub fn to_s(&self) -> Option<String> {
        match self {
            Param::Null => Some(String::new()),
            Param::Bool(b) => Some(b.to_string()),
            Param::Number(n) => Some(n.to_string()),
            Param::Str(s) => Some(s.clone()),
            _ => None,
        }
    }

    /// ActiveSupport's `blank?`: nil, false, whitespace-only strings, and empty collections.
    pub fn is_blank(&self) -> bool {
        match self {
            Param::Null => true,
            Param::Bool(b) => !b,
            Param::Number(_) | Param::File(_) => false,
            Param::Str(s) => s.chars().all(char::is_whitespace),
            Param::Array(items) => items.is_empty(),
            Param::Hash(map) => map.is_empty(),
        }
    }

    pub fn is_present(&self) -> bool {
        !self.is_blank()
    }

    /// The strong-parameters notion of a permitted scalar.
    fn is_permitted_scalar(&self) -> bool {
        !matches!(self, Param::Array(_) | Param::Hash(_))
    }

    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value;
        match self {
            Param::Null => Value::Null,
            Param::Bool(b) => Value::Bool(*b),
            Param::Number(n) => Value::Number(n.clone()),
            Param::Str(s) => Value::String(s.clone()),
            Param::File(file) => serde_json::json!({
                "original_filename": file.original_filename,
                "content_type": file.content_type,
            }),
            Param::Array(items) => Value::Array(items.iter().map(Param::to_json).collect()),
            Param::Hash(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), v.to_json())).collect()),
        }
    }

    /// `ParamBuilder.from_hash` for a decoded JSON body: nils are compacted out of arrays
    /// (`NoNilParamEncoder`, i.e. deep munge).
    pub fn from_json(value: serde_json::Value) -> Param {
        use serde_json::Value;
        match value {
            Value::Null => Param::Null,
            Value::Bool(b) => Param::Bool(b),
            Value::Number(n) => Param::Number(n),
            Value::String(s) => Param::Str(s),
            Value::Array(items) => Param::Array(items.into_iter().filter(|v| !v.is_null()).map(Param::from_json).collect()),
            Value::Object(map) => {
                let mut params = ParamMap::new();
                for (k, v) in map {
                    params.insert(k, Param::from_json(v));
                }
                Param::Hash(params)
            }
        }
    }

    /// `params.require(:key).permit(...)`'s second half: keep only the permitted keys.
    pub fn permit(&self, filters: &[Permit]) -> ParamMap {
        match self {
            Param::Hash(map) => map.permit(filters),
            _ => ParamMap::new(),
        }
    }
}

impl From<&str> for Param {
    fn from(s: &str) -> Self {
        Param::Str(s.to_string())
    }
}

impl From<String> for Param {
    fn from(s: String) -> Self {
        Param::Str(s)
    }
}

/// An insertion-ordered string-keyed map, like the Ruby `Hash` behind Rails params. Indexed, so
/// params with many keys (a large JSON body, a long query string) cost linear time, not quadratic.
#[derive(Debug, Clone, Default)]
pub struct ParamMap {
    entries: indexmap::IndexMap<String, Param>,
}

/// Equal when the same keys map to equal values in the same order, as Ruby hashes compare.
impl PartialEq for ParamMap {
    fn eq(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len() && self.entries.iter().eq(other.entries.iter())
    }
}

impl ParamMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &str) -> Option<&Param> {
        self.entries.get(key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Param> {
        self.entries.get_mut(key)
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Param::as_str)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Ruby's `hash[key] = value`: replaces in place, keeping the original position.
    pub fn insert(&mut self, key: impl Into<String>, value: Param) {
        self.entries.insert(key.into(), value);
    }

    pub fn remove(&mut self, key: &str) -> Option<Param> {
        self.entries.shift_remove(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Param)> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Ruby's `Hash#merge!`: later values win, existing keys keep their position.
    pub fn merge(&mut self, other: &ParamMap) {
        for (k, v) in other.iter() {
            self.insert(k.clone(), v.clone());
        }
    }

    /// `params.require(:key)`: the value when it's present (or `false`), else `ParameterMissing`.
    pub fn require(&self, key: &str) -> crate::Result<&Param> {
        match self.get(key) {
            Some(value) if value.is_present() || matches!(value, Param::Bool(false)) => Ok(value),
            _ => Err(crate::Error::ParameterMissing(key.to_string())),
        }
    }

    /// `params.fetch(:key, default)`.
    pub fn fetch(&self, key: &str, default: Param) -> Param {
        self.get(key).cloned().unwrap_or(default)
    }

    /// `ActionController::Parameters#permit`, for the filter shapes Matchbox uses.
    pub fn permit(&self, filters: &[Permit]) -> ParamMap {
        let mut permitted = ParamMap::new();
        for filter in filters {
            match filter {
                Permit::Key(key) => {
                    if let Some(value) = self.get(key).filter(|v| v.is_permitted_scalar()) {
                        permitted.insert(key.clone(), value.clone());
                    }
                    // Multi-parameter attributes, e.g. `born_on(1i)`.
                    for (k, v) in self.iter() {
                        if is_multi_parameter_key(k, key) && v.is_permitted_scalar() {
                            permitted.insert(k.clone(), v.clone());
                        }
                    }
                }
                Permit::ScalarArray(key) => {
                    if let Some(Param::Array(items)) = self.get(key)
                        && items.iter().all(Param::is_permitted_scalar)
                    {
                        permitted.insert(key.clone(), Param::Array(items.clone()));
                    }
                }
                Permit::AnyHash(key) => {
                    if let Some(Param::Hash(map)) = self.get(key) {
                        permitted.insert(key.clone(), Param::Hash(permit_any(map)));
                    }
                }
                Permit::Nested(key, nested) => match self.get(key) {
                    Some(Param::Hash(map)) if is_fields_for_style(map) => {
                        let mut each = ParamMap::new();
                        for (k, v) in map.iter() {
                            if let Param::Hash(inner) = v {
                                each.insert(k.clone(), Param::Hash(inner.permit(nested)));
                            }
                        }
                        permitted.insert(key.clone(), Param::Hash(each));
                    }
                    Some(Param::Hash(map)) => {
                        permitted.insert(key.clone(), Param::Hash(map.permit(nested)));
                    }
                    Some(Param::Array(items)) => {
                        let hashes = items.iter().filter_map(Param::as_hash).map(|m| Param::Hash(m.permit(nested))).collect();
                        permitted.insert(key.clone(), Param::Array(hashes));
                    }
                    _ => {}
                },
            }
        }
        permitted
    }

    pub fn to_json(&self) -> serde_json::Value {
        Param::Hash(self.clone()).to_json()
    }
}

impl FromIterator<(String, Param)> for ParamMap {
    fn from_iter<T: IntoIterator<Item = (String, Param)>>(iter: T) -> Self {
        let mut map = ParamMap::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

fn is_multi_parameter_key(candidate: &str, key: &str) -> bool {
    // /\A#{key}\(\d+[if]?\)\z/
    let Some(rest) = candidate.strip_prefix(key).and_then(|r| r.strip_prefix('(')).and_then(|r| r.strip_suffix(')')) else {
        return false;
    };
    let digits = rest.strip_suffix(['i', 'f']).unwrap_or(rest);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

fn is_fields_for_style(map: &ParamMap) -> bool {
    !map.is_empty()
        && map.iter().all(|(k, v)| {
            let digits = k.strip_prefix('-').unwrap_or(k);
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && matches!(v, Param::Hash(_))
        })
}

/// `permit(key: {})`: any scalars, arrays of scalars or hashes, recursively.
fn permit_any(map: &ParamMap) -> ParamMap {
    let mut permitted = ParamMap::new();
    for (k, v) in map.iter() {
        match v {
            Param::Hash(inner) => permitted.insert(k.clone(), Param::Hash(permit_any(inner))),
            Param::Array(items) => {
                let kept = items
                    .iter()
                    .filter_map(|item| match item {
                        Param::Hash(inner) => Some(Param::Hash(permit_any(inner))),
                        Param::Array(_) => None,
                        scalar => Some(scalar.clone()),
                    })
                    .collect();
                permitted.insert(k.clone(), Param::Array(kept))
            }
            scalar => permitted.insert(k.clone(), scalar.clone()),
        }
    }
    permitted
}

/// A `permit` filter.
#[derive(Debug, Clone)]
pub enum Permit {
    /// `:name`
    Key(String),
    /// `name: []`
    ScalarArray(String),
    /// `name: {}`
    AnyHash(String),
    /// `name: [ ... ]` / `name: { ... }`
    Nested(String, Vec<Permit>),
}

impl From<&str> for Permit {
    fn from(key: &str) -> Self {
        Permit::Key(key.to_string())
    }
}

/// Shorthand for `permit(&["a".into(), ...])`: `permit_keys(&["name", "avatar"])`.
pub fn permit_keys(keys: &[&str]) -> Vec<Permit> {
    keys.iter().map(|k| Permit::from(*k)).collect()
}

/// A file part of a multipart body (`ActionDispatch::Http::UploadedFile`), spooled to a temp
/// file that's deleted when the last reference drops.
#[derive(Debug)]
pub struct UploadedFile {
    pub original_filename: String,
    pub content_type: Option<String>,
    /// The raw part headers (`UploadedFile#headers`).
    pub headers: String,
    pub size: u64,
    path: tempfile::TempPath,
}

impl UploadedFile {
    pub fn new(original_filename: String, content_type: Option<String>, headers: String, size: u64, path: tempfile::TempPath) -> Self {
        Self { original_filename, content_type, headers, size, path }
    }

    /// Spool `bytes` to a temp file; handy for tests and for bot raw-body attachments.
    pub fn from_bytes(original_filename: &str, content_type: Option<&str>, bytes: &[u8]) -> std::io::Result<Self> {
        use std::io::Write;
        let mut file = tempfile::Builder::new().prefix("RackMultipart").tempfile()?;
        file.write_all(bytes)?;
        Ok(Self {
            original_filename: original_filename.to_string(),
            content_type: content_type.map(str::to_string),
            headers: String::new(),
            size: bytes.len() as u64,
            path: file.into_temp_path(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> std::io::Result<Vec<u8>> {
        std::fs::read(&self.path)
    }
}

impl PartialEq for UploadedFile {
    fn eq(&self, other: &Self) -> bool {
        self.path.as_os_str() == other.path.as_os_str()
    }
}

// ---------------------------------------------------------------------------------------------
// Pair splitting and decoding
// ---------------------------------------------------------------------------------------------

/// A decoded `key=value` pair. `value` is `None` when there was no `=` (Rails gives nil).
pub type Pair = (String, Option<String>);

/// `ActionDispatch::QueryParser.each_pair`: split on `/& */`, skip empty parts, split on the first
/// `=`, and `URI.decode_www_form_component` both halves. No size limits (Rails applies none to
/// the query string).
pub fn query_pairs(qs: &str) -> Result<Vec<RawPair>, ParamError> {
    split_pairs(qs).map(decode_pair).collect()
}

/// `Rack::Request#form_pairs` for an urlencoded body: Rack's limits, and the trailing `\0`
/// Safari once appended is dropped.
pub fn form_pairs(body: &[u8]) -> Result<Vec<RawPair>, ParamError> {
    if body.len() > FORM_BYTESIZE_LIMIT {
        return Err(ParamError::Limit(format!("total query size exceeds limit ({FORM_BYTESIZE_LIMIT})")));
    }
    let body = body.strip_suffix(b"\0").unwrap_or(body);
    // The body is bytes; %-decoding happens per component, so a lossless view is enough here.
    let text = String::from_utf8_lossy(body);
    let parts: Vec<&str> = split_pairs(&text).collect();
    let total = text.split('&').count();
    if total > FORM_PARAMS_LIMIT {
        return Err(ParamError::Limit(format!("total number of query parameters ({total}) exceeds limit ({FORM_PARAMS_LIMIT})")));
    }
    if matches!(text, std::borrow::Cow::Owned(_)) {
        // Raw non-UTF-8 bytes in a form body can't decode to valid UTF-8 params either.
        return Err(ParamError::Invalid("Invalid encoding for parameter".into()));
    }
    parts.into_iter().map(decode_pair).collect()
}

/// A pair after %-decoding but before the UTF-8 check, which Rails performs in the builder
/// (so pairs whose top-level key is empty are skipped without raising).
#[derive(Debug, Clone)]
pub struct RawPair {
    pub key: Vec<u8>,
    pub value: Option<PairValue>,
}

#[derive(Debug, Clone)]
pub enum PairValue {
    Bytes(Vec<u8>),
    File(Arc<UploadedFile>),
}

impl RawPair {
    pub fn text(key: &str, value: Option<&str>) -> Self {
        Self { key: key.as_bytes().to_vec(), value: value.map(|v| PairValue::Bytes(v.as_bytes().to_vec())) }
    }

    pub fn file(key: &str, file: UploadedFile) -> Self {
        Self { key: key.as_bytes().to_vec(), value: Some(PairValue::File(Arc::new(file))) }
    }
}

fn split_pairs(qs: &str) -> impl Iterator<Item = &str> {
    qs.split('&').enumerate().map(|(i, part)| if i == 0 { part } else { part.trim_start_matches(' ') }).filter(|p| !p.is_empty())
}

fn decode_pair(part: &str) -> Result<RawPair, ParamError> {
    let (k, v) = match part.split_once('=') {
        Some((k, v)) => (k, Some(v)),
        None => (part, None),
    };
    Ok(RawPair { key: decode_www_form_component(k)?, value: v.map(decode_www_form_component).transpose()?.map(PairValue::Bytes) })
}

/// Ruby's `URI.decode_www_form_component`: `+` is a space, `%XX` is a byte, and a `%` not
/// followed by two hex digits is an error.
pub fn decode_www_form_component(s: &str) -> Result<Vec<u8>, ParamError> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hex = bytes.get(i + 1..i + 3).filter(|h| h.iter().all(u8::is_ascii_hexdigit));
                let Some(hex) = hex else {
                    return Err(ParamError::Invalid(format!("invalid %-encoding ({s})")));
                };
                out.push(u8::from_str_radix(std::str::from_utf8(hex).unwrap(), 16).unwrap());
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// ParamBuilder
// ---------------------------------------------------------------------------------------------

/// `ActionDispatch::ParamBuilder.from_pairs`.
pub fn from_pairs(pairs: impl IntoIterator<Item = RawPair>) -> Result<ParamMap, ParamError> {
    let mut params = ParamMap::new();
    for pair in pairs {
        let RawPair { key, value } = pair;
        let key = match String::from_utf8(key) {
            Ok(key) => key,
            Err(err) => {
                if top_level_key(&String::from_utf8_lossy(err.as_bytes())).is_empty() {
                    continue;
                }
                return Err(ParamError::Invalid("Invalid encoding for parameter".into()));
            }
        };
        if top_level_key(&key).is_empty() {
            continue;
        }
        let value = match value {
            None => Param::Null,
            Some(PairValue::File(file)) => Param::File(file),
            Some(PairValue::Bytes(bytes)) => match String::from_utf8(bytes) {
                Ok(s) => Param::Str(s),
                Err(err) => {
                    return Err(ParamError::Invalid(format!(
                        "Invalid encoding for parameter: {}",
                        String::from_utf8_lossy(err.as_bytes())
                    )));
                }
            },
        };
        store_nested_param(&mut params, &key, value, 0)?;
    }
    Ok(params)
}

/// `ActionDispatch::ParamBuilder.from_query_string`.
pub fn from_query_string(qs: &str) -> Result<ParamMap, ParamError> {
    from_pairs(query_pairs(qs)?)
}

/// The query-string parser as JSON, for differential tests against
/// `ActionDispatch::ParamBuilder.from_query_string(qs)` in the reference container. Returns
/// `null` when Rails would raise (a 400); see [`try_parse_nested`] for the error itself.
pub fn parse_nested(qs: &str) -> serde_json::Value {
    try_parse_nested(qs).unwrap_or(serde_json::Value::Null)
}

pub fn try_parse_nested(qs: &str) -> Result<serde_json::Value, ParamError> {
    Ok(from_query_string(qs)?.to_json())
}

fn top_level_key(name: &str) -> &str {
    match find_byte(name, b'[', 1) {
        Some(start) => &name[..start],
        None => name,
    }
}

fn find_byte(s: &str, byte: u8, from: usize) -> Option<usize> {
    s.as_bytes().iter().enumerate().skip(from).find(|(_, b)| **b == byte).map(|(i, _)| i)
}

/// What `store_nested_param` returned: the params hash it was given, a one-element array (for a
/// trailing `[]` below the top level), or nil (for an empty key).
enum Stored {
    Params,
    Array(Vec<Param>),
    Nil,
}

impl Stored {
    fn into_param(self, params: ParamMap) -> Param {
        match self {
            Stored::Params => Param::Hash(params),
            Stored::Array(items) => Param::Array(items),
            Stored::Nil => Param::Null,
        }
    }
}

fn store_nested_param(params: &mut ParamMap, name: &str, v: Param, depth: usize) -> Result<Stored, ParamError> {
    if depth >= DEPTH_LIMIT {
        return Err(ParamError::TooDeep);
    }

    let (k, after): (&str, &str) = if depth == 0 {
        // Start of parsing, don't treat [] or [ at start of string specially.
        match find_byte(name, b'[', 1) {
            Some(start) => (&name[..start], &name[start..]),
            None => (name, ""),
        }
    } else if let Some(rest) = name.strip_prefix("[]") {
        ("[]", rest)
    } else if let Some(start) = name.starts_with('[').then(|| find_byte(name, b']', 1)).flatten() {
        (&name[1..start], &name[start + 1..])
    } else {
        // Probably malformed input, nested but not starting with [.
        (name, "")
    };

    if k.is_empty() {
        return Ok(Stored::Nil);
    }

    if after.is_empty() {
        if k == "[]" && depth != 0 {
            return Ok(Stored::Array(if v.is_null() { vec![] } else { vec![v] }));
        }
        params.insert(k, v);
    } else if after == "[" {
        params.insert(name, v);
    } else if after == "[]" {
        let array = array_slot(params, k)?;
        if !v.is_null() {
            array.push(v);
        }
    } else if let Some(nested) = after.strip_prefix("[]") {
        // Recognize x[][y] (hash inside array) parameters; otherwise nest what follows the [].
        let child_key = match nested.strip_prefix('[').and_then(|n| n.strip_suffix(']')) {
            Some(inner) if !inner.is_empty() && !inner.contains('[') && !inner.contains(']') => inner,
            _ => nested,
        };
        let array = array_slot(params, k)?;
        match array.last_mut() {
            Some(Param::Hash(last)) if !params_hash_has_key(last, child_key) => {
                store_nested_param(last, child_key, v, depth + 1)?;
            }
            _ => {
                let mut child = ParamMap::new();
                let stored = store_nested_param(&mut child, child_key, v, depth + 1)?;
                array.push(stored.into_param(child));
            }
        }
    } else {
        let mut child = match params.get_mut(k).map(|slot| std::mem::replace(slot, Param::Null)) {
            None | Some(Param::Null) => ParamMap::new(),
            Some(Param::Hash(existing)) => existing,
            Some(other) => {
                return Err(ParamError::Type(format!("expected Hash (got {}) for param `{k}'", ruby_class(&other))));
            }
        };
        let stored = store_nested_param(&mut child, after, v, depth + 1)?;
        params.insert(k, stored.into_param(child));
    }

    Ok(Stored::Params)
}

/// `params[k] ||= []` followed by the Array type check.
fn array_slot<'a>(params: &'a mut ParamMap, k: &str) -> Result<&'a mut Vec<Param>, ParamError> {
    if params.get(k).is_none_or(Param::is_null) {
        params.insert(k, Param::Array(vec![]));
    }
    match params.get_mut(k) {
        Some(Param::Array(items)) => Ok(items),
        Some(other) => Err(ParamError::Type(format!("expected Array (got {}) for param `{k}'", ruby_class(other)))),
        None => unreachable!(),
    }
}

fn params_hash_has_key(hash: &ParamMap, key: &str) -> bool {
    if key.contains("[]") {
        return false;
    }
    // Ruby's `key.split(/[\[\]]+/)`; empty parts are skipped.
    let mut current = Some(hash);
    for part in key.split(['[', ']']).filter(|p| !p.is_empty()) {
        let Some(map) = current else { return false };
        match map.get(part) {
            Some(value) => current = value.as_hash(),
            None => return false,
        }
    }
    true
}

fn ruby_class(param: &Param) -> &'static str {
    match param {
        Param::Null => "NilClass",
        Param::Bool(true) => "TrueClass",
        Param::Bool(false) => "FalseClass",
        Param::Number(_) => "Integer",
        Param::Str(_) => "String",
        Param::File(_) => "ActionDispatch::Http::UploadedFile",
        Param::Array(_) => "Array",
        Param::Hash(_) => "ActiveSupport::HashWithIndifferentAccess",
    }
}

/// `ParamBuilder.from_hash` for a JSON body: `Parameters::DEFAULT_PARSERS[:json]` wraps non-hash
/// documents as `{ "_json" => data }`.
pub fn from_json_body(body: &[u8]) -> Result<ParamMap, ParamError> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| ParamError::Parse)?;
    Ok(match Param::from_json(value) {
        Param::Hash(map) => map,
        other => {
            let mut map = ParamMap::new();
            map.insert("_json", other);
            map
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(qs: &str) -> serde_json::Value {
        try_parse_nested(qs).unwrap()
    }

    #[test]
    fn flat_pairs() {
        assert_eq!(parse("a=1&b=2"), json!({"a": "1", "b": "2"}));
        assert_eq!(parse(""), json!({}));
        assert_eq!(parse("a"), json!({"a": null}));
        assert_eq!(parse("a="), json!({"a": ""}));
        assert_eq!(parse("a=1&a=2"), json!({"a": "2"}));
        assert_eq!(parse("&&a=1&"), json!({"a": "1"}));
        assert_eq!(parse("a=1&  b=2"), json!({"a": "1", "b": "2"}));
        assert_eq!(parse("=1"), json!({}));
        assert_eq!(parse("a=b=c"), json!({"a": "b=c"}));
        assert_eq!(parse("a;b=1"), json!({"a;b": "1"}));
    }

    #[test]
    fn decoding() {
        assert_eq!(parse("a=x+y%20z"), json!({"a": "x y z"}));
        assert_eq!(parse("a%5Bb%5D=1"), json!({"a": {"b": "1"}}));
        assert_eq!(parse("caf%C3%A9=%E2%9C%93"), json!({"café": "✓"}));
        assert!(matches!(try_parse_nested("a=%"), Err(ParamError::Invalid(_))));
        assert!(matches!(try_parse_nested("a=%zz"), Err(ParamError::Invalid(_))));
        assert!(matches!(try_parse_nested("a=%FF"), Err(ParamError::Invalid(_))));
        // An empty top-level key is skipped before the encoding check.
        assert_eq!(parse("=%FF"), json!({}));
    }

    #[test]
    fn nested_hashes() {
        assert_eq!(parse("a[b]=1"), json!({"a": {"b": "1"}}));
        assert_eq!(parse("a[b][c]=1&a[b][d]=2"), json!({"a": {"b": {"c": "1", "d": "2"}}}));
        assert_eq!(parse("a[b]c=1"), json!({"a": {"b": {"c": "1"}}}));
        assert_eq!(parse("[a]=1"), json!({"[a]": "1"}));
        assert_eq!(parse("a[=1"), json!({"a[": "1"}));
        assert_eq!(parse("a[b=1"), json!({"a": {"[b": "1"}}));
        assert_eq!(parse("a]=1"), json!({"a]": "1"}));
        assert_eq!(parse("a[]]=1"), json!({"a": [{"]": "1"}]}));
        assert_eq!(parse("a[[]]=1"), json!({"a": {"[": {"]": "1"}}}));
    }

    #[test]
    fn arrays() {
        assert_eq!(parse("a[]=1&a[]=2"), json!({"a": ["1", "2"]}));
        assert_eq!(parse("a[]"), json!({"a": []}));
        assert_eq!(parse("a[]=&a[]"), json!({"a": [""]}));
        assert_eq!(parse("a[][]=1"), json!({"a": [["1"]]}));
        assert_eq!(parse("a[][]"), json!({"a": [[]]}));
        assert_eq!(parse("a[b][]=1&a[b][]=2"), json!({"a": {"b": ["1", "2"]}}));
    }

    #[test]
    fn hashes_inside_arrays() {
        assert_eq!(parse("a[][b]=1&a[][c]=2&a[][b]=3"), json!({"a": [{"b": "1", "c": "2"}, {"b": "3"}]}));
        assert_eq!(parse("a[][b][c]=1&a[][b][d]=2"), json!({"a": [{"b": {"c": "1", "d": "2"}}]}));
        assert_eq!(parse("a[][b][c]=1&a[][b][c]=2"), json!({"a": [{"b": {"c": "1"}}, {"b": {"c": "2"}}]}));
        assert_eq!(parse("a[][b][]=1&a[][b][]=2"), json!({"a": [{"b": ["1", "2"]}]}));
        assert_eq!(parse("a[]b=1"), json!({"a": [{"b": "1"}]}));
        assert_eq!(parse("a[][b]"), json!({"a": [{"b": null}]}));
    }

    #[test]
    fn nil_replaced_by_structure() {
        // `||=` treats nil as unset.
        assert_eq!(parse("a&a[]=1"), json!({"a": ["1"]}));
        assert_eq!(parse("a&a[b]=1"), json!({"a": {"b": "1"}}));
    }

    #[test]
    fn type_conflicts() {
        assert!(matches!(try_parse_nested("a=1&a[]=2"), Err(ParamError::Type(_))));
        assert!(matches!(try_parse_nested("a=1&a[b]=2"), Err(ParamError::Type(_))));
        assert!(matches!(try_parse_nested("a[]=1&a[b]=2"), Err(ParamError::Type(_))));
        assert!(matches!(try_parse_nested("a[b]=1&a[]=2"), Err(ParamError::Type(_))));
        assert!(matches!(try_parse_nested("a[b]=1&a[b][c]=2"), Err(ParamError::Type(_))));
        assert_eq!(parse_nested("a=1&a[]=2"), serde_json::Value::Null);
    }

    #[test]
    fn depth_limit() {
        let deep = format!("a{}=1", "[b]".repeat(DEPTH_LIMIT - 1));
        assert!(try_parse_nested(&deep).is_ok());
        let too_deep = format!("a{}=1", "[b]".repeat(DEPTH_LIMIT));
        assert_eq!(try_parse_nested(&too_deep), Err(ParamError::TooDeep));
    }

    #[test]
    fn form_body_limits() {
        assert_eq!(from_pairs(form_pairs(b"a=1\0").unwrap()).unwrap().to_json(), json!({"a": "1"}));
        let many = vec!["a=1"; FORM_PARAMS_LIMIT + 1].join("&");
        assert!(matches!(form_pairs(many.as_bytes()), Err(ParamError::Limit(_))));
    }

    #[test]
    fn json_bodies_are_deep_munged() {
        let params = from_json_body(br#"{"a": [1, null, "x"], "b": {"c": null}, "d": true}"#).unwrap();
        assert_eq!(params.to_json(), json!({"a": [1, "x"], "b": {"c": null}, "d": true}));
        assert_eq!(from_json_body(b"[1,2]").unwrap().to_json(), json!({"_json": [1, 2]}));
        assert_eq!(from_json_body(b"{bad"), Err(ParamError::Parse));
    }

    #[test]
    fn require_and_permit() {
        let params = from_query_string(
            "user[name]=Jo&user[admin]=1&user[tags][]=a&user[settings][x][y]=1&user[bad][]=1&blank=+&user[date(1i)]=2024",
        )
        .unwrap();
        assert!(params.require("blank").is_err());
        assert!(params.require("missing").is_err());
        let user = params.require("user").unwrap();
        let permitted = user.permit(&[
            "name".into(),
            "date".into(),
            Permit::ScalarArray("tags".into()),
            Permit::AnyHash("settings".into()),
            "bad".into(),
        ]);
        assert_eq!(permitted.to_json(), json!({"name": "Jo", "date(1i)": "2024", "tags": ["a"], "settings": {"x": {"y": "1"}}}));
    }

    #[test]
    fn permit_nested() {
        let params = from_query_string("a[b][c]=1&a[b][d]=2&list[][c]=1&list[][d]=2&ff[0][c]=1&ff[1][c]=2").unwrap();
        let nested = vec![Permit::Key("c".into())];
        let permitted = params.permit(&[
            Permit::Nested("a".into(), vec![Permit::Nested("b".into(), nested.clone())]),
            Permit::Nested("list".into(), nested.clone()),
            Permit::Nested("ff".into(), nested),
        ]);
        assert_eq!(permitted.to_json(), json!({"a": {"b": {"c": "1"}}, "list": [{"c": "1"}], "ff": {"0": {"c": "1"}, "1": {"c": "2"}}}));
    }

    #[test]
    fn fetch_default() {
        let params = from_query_string("user_ids[]=1&user_ids[]=2").unwrap();
        assert_eq!(params.fetch("user_ids", Param::Array(vec![])).to_json(), json!(["1", "2"]));
        assert_eq!(params.fetch("nope", Param::Array(vec![])).to_json(), json!([]));
    }

    #[test]
    fn many_keys_build_in_linear_time() {
        let object: serde_json::Map<String, serde_json::Value> = (0..100_000).map(|n| (format!("k{n}"), n.into())).collect();
        let started = std::time::Instant::now();
        let Param::Hash(map) = Param::from_json(serde_json::Value::Object(object)) else { panic!("a hash") };
        assert_eq!(map.len(), 100_000);
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
    }
}
