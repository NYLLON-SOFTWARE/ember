//! `RestrictedHTTP::PrivateNetworkGuard` (reference/lib/restricted_http/private_network_guard.rb)
//! and the parts of the surfguard gem it calls (`Surfguard.resolve_public_ips`,
//! `Surfguard.blocked_address?`, gem revision 59e278c, default policy).
//!
//! A host goes in and a public address to pin comes out. Numeric hosts never reach DNS; names
//! must be plain LDH labels. Every answer is classified and the blocked ones dropped; IPv4
//! answers come before IPv6 ones, in resolver order within each family.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::Resolver;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GuardError {
    /// `RestrictedHTTP::Violation`: the host only resolves to blocked addresses (or is malformed).
    #[error("Attempt to access private IP via {0}")]
    Violation(String),
    /// `Surfguard::Unresolvable`: the lookup failed or came back empty.
    #[error("Host could not be resolved")]
    Unresolvable,
}

/// `RestrictedHTTP::PrivateNetworkGuard.resolve(hostname)`: the first public address.
pub async fn resolve(resolver: &dyn Resolver, host: &str) -> Result<IpAddr, GuardError> {
    resolve_public_ips(resolver, host).await?.into_iter().next().ok_or_else(|| GuardError::Violation(host.to_string()))
}

/// `Surfguard.resolve_public_ips(host)`. Malformed input comes back empty rather than raising.
pub async fn resolve_public_ips(resolver: &dyn Resolver, host: &str) -> Result<Vec<IpAddr>, GuardError> {
    if !normal_host(host) {
        return Ok(Vec::new());
    }
    let addresses = match numeric_literals(host) {
        Numeric::Invalid => return Ok(Vec::new()),
        Numeric::Literal(addresses) => addresses,
        Numeric::Name => resolver.lookup(host).await.map_err(|_| GuardError::Unresolvable)?,
    };
    let addresses = normalize_answers(addresses)?;
    let (v4, v6): (Vec<IpAddr>, Vec<IpAddr>) = addresses.into_iter().filter(|ip| !blocked_address(*ip)).partition(IpAddr::is_ipv4);
    Ok(v4.into_iter().chain(v6).collect())
}

const MAX_HOST_BYTES: usize = 255;
const MAX_ADDRESSES: usize = 256;

/// `normalize_host` for a String host: ASCII, no NUL, not empty, at most 255 bytes, no zone.
fn normal_host(host: &str) -> bool {
    host.is_ascii() && !host.contains('\0') && !host.is_empty() && host.len() <= MAX_HOST_BYTES && !host.contains('%')
}

/// `normalize_answers`: at most 256 answers, deduplicated, and at least one.
fn normalize_answers(raw: Vec<IpAddr>) -> Result<Vec<IpAddr>, GuardError> {
    if raw.len() > MAX_ADDRESSES {
        return Err(GuardError::Unresolvable);
    }
    let mut answers: Vec<IpAddr> = Vec::new();
    for ip in raw {
        if !answers.contains(&ip) {
            answers.push(ip);
        }
    }
    if answers.is_empty() { Err(GuardError::Unresolvable) } else { Ok(answers) }
}

enum Numeric {
    /// Surfguard raises `InvalidInput`: the caller gets no addresses.
    Invalid,
    Literal(Vec<IpAddr>),
    /// Not numeric: ask the resolver.
    Name,
}

/// `numeric_literals`: `getaddrinfo(host, AI_NUMERICHOST)`, then an `IPAddr` literal (which
/// accepts brackets), else a name for DNS unless it looks numeric.
fn numeric_literals(host: &str) -> Numeric {
    if !valid_host_syntax(host) || malformed_numeric_host_candidate(host) {
        return Numeric::Invalid;
    }
    if let Some(ip) = getaddrinfo_numeric(host) {
        return Numeric::Literal(vec![ip]);
    }
    if let Some(ip) = ip_literal(host) {
        return Numeric::Literal(vec![ip]);
    }
    if numeric_host_candidate(host) { Numeric::Invalid } else { Numeric::Name }
}

fn valid_host_syntax(host: &str) -> bool {
    if host.contains(':') || legacy_ipv4_shape(host) || full_width_host_literal(host) {
        return true;
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    let labels: Vec<&str> = host.split('.').collect();
    labels.iter().all(|label| {
        let bytes = label.as_bytes();
        !bytes.is_empty()
            && bytes.len() <= 63
            && bytes[0].is_ascii_alphanumeric()
            && bytes[bytes.len() - 1].is_ascii_alphanumeric()
            && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-')
    })
}

fn numeric_host_candidate(host: &str) -> bool {
    host.contains(':') || legacy_ipv4_shape(host)
}

fn malformed_numeric_host_candidate(host: &str) -> bool {
    if host.contains(':') {
        return false;
    }
    let core = host.trim_start_matches(['%', '/']);
    let core = &core[..core.find(['%', '/']).unwrap_or(core.len())];
    let malformed = core != host || core.split('.').any(str::is_empty);
    malformed && legacy_ipv4_shape(core) && !full_width_host_literal(host)
}

/// 1 to 4 dot-separated (empty parts ignored) decimal or 0x-hex numbers.
fn legacy_ipv4_shape(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').filter(|p| !p.is_empty()).collect();
    (1..=4).contains(&parts.len())
        && parts.iter().all(|part| match part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            Some(hex) => !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()),
            None => part.bytes().all(|b| b.is_ascii_digit()),
        })
}

/// `host_address?(IPAddr.new(text))`: a full-length address literal.
fn full_width_host_literal(text: &str) -> bool {
    ip_literal(text).is_some()
}

/// `IPAddr.new(text)` for a single address: dotted-quad IPv4, or IPv6 with optional brackets.
fn ip_literal(text: &str) -> Option<IpAddr> {
    let inner = text.strip_prefix('[').and_then(|t| t.strip_suffix(']'));
    if let Some(inner) = inner {
        return inner.parse::<Ipv6Addr>().ok().map(IpAddr::V6);
    }
    if text.contains(':') {
        return text.parse::<Ipv6Addr>().ok().map(IpAddr::V6);
    }
    // IPAddr only takes four decimal octets without leading zeros ("01" is rejected)
    let octets: Vec<&str> = text.split('.').collect();
    if octets.len() == 4
        && octets.iter().all(|o| !o.is_empty() && o.bytes().all(|b| b.is_ascii_digit()) && (o.len() == 1 || !o.starts_with('0')))
    {
        return text.parse::<Ipv4Addr>().ok().map(IpAddr::V4);
    }
    None
}

/// glibc's `getaddrinfo(..., AI_NUMERICHOST)`: `inet_aton` forms for IPv4, `inet_pton` for IPv6.
fn getaddrinfo_numeric(host: &str) -> Option<IpAddr> {
    if host.contains(':') {
        return host.parse::<Ipv6Addr>().ok().map(IpAddr::V6);
    }
    inet_aton(host).map(IpAddr::V4)
}

/// glibc `__inet_aton_exact`: 1-4 parts in decimal, octal (leading 0) or hex (0x); the last
/// part fills the remaining bytes.
fn inet_aton(text: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut values = Vec::with_capacity(parts.len());
    for part in &parts {
        let (digits, radix) = if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            (hex, 16)
        } else if part.len() > 1 && part.starts_with('0') {
            (&part[1..], 8)
        } else {
            (*part, 10)
        };
        if part.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return None;
        }
        let value = if digits.is_empty() { 0 } else { u64::from_str_radix(digits, radix).ok()? };
        if value > u32::MAX as u64 {
            return None;
        }
        values.push(value as u32);
    }
    let (last, leading) = values.split_last()?;
    if leading.iter().any(|v| *v > 0xff) {
        return None;
    }
    let remaining_bits = 32 - 8 * leading.len() as u32;
    if remaining_bits < 32 && *last >= 1 << remaining_bits {
        return None;
    }
    let mut address = *last;
    for (i, v) in leading.iter().enumerate() {
        address |= v << (24 - 8 * i as u32);
    }
    Some(Ipv4Addr::from(address))
}

// --- Classification (Surfguard.blocked_address?, default policy) ---------------------------------

type V4Range = (u32, u8);
type V6Range = (u128, u8);

const fn v4(a: u8, b: u8, c: u8, d: u8, prefix: u8) -> V4Range {
    (u32::from_be_bytes([a, b, c, d]), prefix)
}

const fn v6(segments: [u16; 8], prefix: u8) -> V6Range {
    let mut value: u128 = 0;
    let mut i = 0;
    while i < 8 {
        value = (value << 16) | segments[i] as u128;
        i += 1;
    }
    (value, prefix)
}

/// `Surfguard::DISALLOWED_IPV4`
const DISALLOWED_IPV4: &[V4Range] = &[
    v4(0, 0, 0, 0, 8),
    v4(10, 0, 0, 0, 8),
    v4(100, 64, 0, 0, 10),
    v4(127, 0, 0, 0, 8),
    v4(168, 63, 129, 16, 32),
    v4(169, 254, 0, 0, 16),
    v4(172, 16, 0, 0, 12),
    v4(192, 0, 0, 0, 24),
    v4(192, 0, 2, 0, 24),
    v4(192, 88, 99, 0, 24),
    v4(192, 168, 0, 0, 16),
    v4(198, 18, 0, 0, 15),
    v4(198, 51, 100, 0, 24),
    v4(203, 0, 113, 0, 24),
    v4(224, 0, 0, 0, 4),
    v4(240, 0, 0, 0, 4),
];

/// `Surfguard::DISALLOWED_IPV6`
const DISALLOWED_IPV6: &[V6Range] = &[
    v6([0, 0, 0, 0, 0, 0, 0, 0], 128),
    v6([0x100, 0, 0, 0, 0, 0, 0, 0], 64),
    v6([0x100, 0, 0, 1, 0, 0, 0, 0], 64),
    v6([0x2001, 0, 0, 0, 0, 0, 0, 0], 32),
    v6([0x2001, 2, 0, 0, 0, 0, 0, 0], 48),
    v6([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32),
    v6([0x2002, 0, 0, 0, 0, 0, 0, 0], 16),
    v6([0x3fff, 0, 0, 0, 0, 0, 0, 0], 20),
    v6([0x5f00, 0, 0, 0, 0, 0, 0, 0], 16),
    v6([0xfec0, 0, 0, 0, 0, 0, 0, 0], 10),
    v6([0xff00, 0, 0, 0, 0, 0, 0, 0], 8),
];

/// `Surfguard::IANA_ALLOCATED_IPV6_UNICAST`
const IANA_ALLOCATED_IPV6_UNICAST: &[V6Range] = &[
    v6([0x2001, 0, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x200, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x400, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x600, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x800, 0, 0, 0, 0, 0, 0], 22),
    v6([0x2001, 0xc00, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0xe00, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x1200, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x1400, 0, 0, 0, 0, 0, 0], 22),
    v6([0x2001, 0x1800, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x1a00, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x1c00, 0, 0, 0, 0, 0, 0], 22),
    v6([0x2001, 0x2000, 0, 0, 0, 0, 0, 0], 19),
    v6([0x2001, 0x4000, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x4200, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x4400, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x4600, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x4800, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x4a00, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x4c00, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2001, 0x5000, 0, 0, 0, 0, 0, 0], 20),
    v6([0x2001, 0x8000, 0, 0, 0, 0, 0, 0], 19),
    v6([0x2001, 0xa000, 0, 0, 0, 0, 0, 0], 20),
    v6([0x2001, 0xb000, 0, 0, 0, 0, 0, 0], 20),
    v6([0x2002, 0, 0, 0, 0, 0, 0, 0], 16),
    v6([0x2003, 0, 0, 0, 0, 0, 0, 0], 18),
    v6([0x2400, 0, 0, 0, 0, 0, 0, 0], 12),
    v6([0x2410, 0, 0, 0, 0, 0, 0, 0], 12),
    v6([0x2600, 0, 0, 0, 0, 0, 0, 0], 12),
    v6([0x2610, 0, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2620, 0, 0, 0, 0, 0, 0, 0], 23),
    v6([0x2630, 0, 0, 0, 0, 0, 0, 0], 12),
    v6([0x2800, 0, 0, 0, 0, 0, 0, 0], 12),
    v6([0x2a00, 0, 0, 0, 0, 0, 0, 0], 12),
    v6([0x2a10, 0, 0, 0, 0, 0, 0, 0], 12),
    v6([0x2c00, 0, 0, 0, 0, 0, 0, 0], 12),
];

/// `Surfguard::GLOBALLY_REACHABLE_IETF_ASSIGNMENTS`
const GLOBALLY_REACHABLE_IETF_ASSIGNMENTS: &[V6Range] = &[v6([0x2001, 3, 0, 0, 0, 0, 0, 0], 32), v6([0x2001, 4, 0x112, 0, 0, 0, 0, 0], 48)];
const IETF_PROTOCOL_ASSIGNMENTS: V6Range = v6([0x2001, 0, 0, 0, 0, 0, 0, 0], 23);
const NAT64_WELL_KNOWN: V6Range = v6([0x64, 0xff9b, 0, 0, 0, 0, 0, 0], 96);
const NAT64_LOCAL_USE: V6Range = v6([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48);
const IPV4_MAPPED: V6Range = v6([0, 0, 0, 0, 0, 0xffff, 0, 0], 96);
const IPV4_TRANSLATABLE: V6Range = v6([0, 0, 0, 0, 0xffff, 0, 0, 0], 96);
const IPV4_COMPATIBLE: V6Range = v6([0, 0, 0, 0, 0, 0, 0, 0], 96);
const UNIQUE_LOCAL: V6Range = v6([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7);
const LINK_LOCAL_V6: V6Range = v6([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10);

fn in_v4(ip: u32, (network, prefix): V4Range) -> bool {
    prefix == 0 || (ip ^ network) >> (32 - prefix as u32) == 0
}

fn in_v6(ip: u128, (network, prefix): V6Range) -> bool {
    prefix == 0 || (ip ^ network) >> (128 - prefix as u32) == 0
}

/// `Surfguard.blocked_address?(ip)`
pub fn blocked_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => disallowed_ipv4(u32::from(ip)),
        IpAddr::V6(ip) => {
            let ip = u128::from(ip);
            if in_v6(ip, IPV4_MAPPED) || in_v6(ip, IPV4_COMPATIBLE) || in_v6(ip, NAT64_LOCAL_USE) {
                true
            } else if in_v6(ip, NAT64_WELL_KNOWN) || in_v6(ip, IPV4_TRANSLATABLE) {
                disallowed_ipv4(ip as u32)
            } else {
                disallowed_ipv6(ip)
            }
        }
    }
}

/// `disallowed_ipv4?`: `private?`, `loopback?` and `link_local?` are all inside the list.
fn disallowed_ipv4(ip: u32) -> bool {
    DISALLOWED_IPV4.iter().any(|range| in_v4(ip, *range))
}

fn disallowed_ipv6(ip: u128) -> bool {
    if GLOBALLY_REACHABLE_IETF_ASSIGNMENTS.iter().any(|range| in_v6(ip, *range)) {
        return false;
    }
    if in_v6(ip, UNIQUE_LOCAL) || ip == 1 || in_v6(ip, LINK_LOCAL_V6) || in_v6(ip, IETF_PROTOCOL_ASSIGNMENTS) {
        return true;
    }
    if DISALLOWED_IPV6.iter().any(|range| in_v6(ip, *range)) {
        return true;
    }
    !IANA_ALLOCATED_IPV6_UNICAST.iter().any(|range| in_v6(ip, *range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::test_support::FakeResolver;

    fn blocked(ip: &str) -> bool {
        blocked_address(ip.parse().unwrap())
    }

    #[test]
    fn classifies_addresses_like_surfguard() {
        for ip in [
            "0.0.0.0", "10.1.2.3", "100.64.0.1", "127.0.0.1", "168.63.129.16", "169.254.169.254", "172.16.0.0", "172.31.255.255",
            "192.0.0.8", "192.0.2.1", "192.88.99.1", "192.168.1.1", "198.18.0.1", "198.51.100.1", "203.0.113.1", "224.0.0.1", "240.0.0.1",
            "255.255.255.255", "::", "::1", "::ffff:192.168.1.1", "::ffff:8.8.8.8", "::8.8.8.8", "64:ff9b::a00:1", "64:ff9b:1::1",
            "::ffff:0:a00:1", "fc00::1", "fd00::1", "fe80::1", "fec0::1", "ff02::1", "2001::1", "2001:db8::1", "2002::1", "3fff::1",
            "5f00::1", "100::1", "2001:2::1", "4000::1", "2001:10::1",
        ] {
            assert!(blocked(ip), "{ip} should be blocked");
        }
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "93.184.216.34",
            "142.250.185.206",
            "172.32.0.1",
            "100.128.0.1",
            "192.0.1.1",
            "2606:2800:220:1:248:1893:25c8:1946",
            "2a00:1450:4001:82a::200e",
            "2001:3::1",
            "2001:4:112::1",
            "64:ff9b::808:808",
            "::ffff:0:808:808",
            "2c0f:ffff::1",
        ] {
            assert!(!blocked(ip), "{ip} should be public");
        }
    }

    #[test]
    fn inet_aton_forms() {
        assert_eq!(inet_aton("127.1"), Some(Ipv4Addr::new(127, 0, 0, 1)));
        assert_eq!(inet_aton("0x7f.1"), Some(Ipv4Addr::new(127, 0, 0, 1)));
        assert_eq!(inet_aton("2130706433"), Some(Ipv4Addr::new(127, 0, 0, 1)));
        assert_eq!(inet_aton("0177.0.0.01"), Some(Ipv4Addr::new(127, 0, 0, 1)));
        assert_eq!(inet_aton("10.0.258"), Some(Ipv4Addr::new(10, 0, 1, 2)));
        assert_eq!(inet_aton("09.1.1.1"), None);
        assert_eq!(inet_aton("256.1.1.1"), None);
        assert_eq!(inet_aton("1.2.3.4."), None);
        assert_eq!(inet_aton("www.example.com"), None);
    }

    #[tokio::test]
    async fn resolves_hosts_like_the_private_network_guard() {
        let resolver = FakeResolver::new([
            ("www.example.com", vec!["93.184.216.34"]),
            ("mixed.example", vec!["10.0.0.1", "2606:2800:220:1:248:1893:25c8:1946", "::1", "93.184.216.39"]),
            ("private.example", vec!["192.168.1.10"]),
        ]);
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert_eq!(resolve(&resolver, "www.example.com").await, Ok(ip("93.184.216.34")));
        assert_eq!(
            resolve_public_ips(&resolver, "mixed.example").await,
            Ok(vec![ip("93.184.216.39"), ip("2606:2800:220:1:248:1893:25c8:1946")])
        );
        assert_eq!(resolve(&resolver, "private.example").await, Err(GuardError::Violation("private.example".into())));
        assert_eq!(resolve(&resolver, "nowhere.example").await, Err(GuardError::Unresolvable));
        assert_eq!(resolve(&resolver, "8.8.8.8").await, Ok(ip("8.8.8.8")));
        assert_eq!(resolve(&resolver, "[2606:2800:220:1:248:1893:25c8:1946]").await, Ok(ip("2606:2800:220:1:248:1893:25c8:1946")));
        for host in [
            "127.0.0.1", "0x7f.1", "2130706433", "[::1]", "::1", "[fd00::1]", "under_score.example", "", "a..b", "1.2.3.4.", "01.2.3.4.",
            "host%eth0", "exämple.com", "-lead.example", "[v1.x]",
        ] {
            assert!(matches!(resolve(&resolver, host).await, Err(GuardError::Violation(_))), "{host:?}");
        }
        assert_eq!(resolver.lookups(), vec!["www.example.com", "mixed.example", "private.example", "nowhere.example"]);
    }
}
