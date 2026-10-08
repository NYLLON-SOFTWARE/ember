//! Canonical Lucide icons generated from the pinned official static icon package.

/// A trusted, locally bundled SVG and its stable picker identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LucideIcon {
    pub name: &'static str,
    pub label: &'static str,
    pub svg: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/lucide.rs"));

/// Every canonical icon in name order; deprecated aliases are omitted.
pub fn lucide_icons() -> &'static [LucideIcon] {
    LUCIDE_ICONS
}

/// Look up an exact canonical name without allocating or parsing the catalog at runtime.
pub fn lucide_icon(name: &str) -> Option<&'static LucideIcon> {
    let index = LUCIDE_ICONS.binary_search_by_key(&name, |icon| icon.name).ok()?;
    Some(&LUCIDE_ICONS[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_icons_have_stable_names_and_safe_svg() {
        let icons = lucide_icons();
        assert_eq!(icons.len(), 1869);
        assert!(icons.windows(2).all(|pair| pair[0].name < pair[1].name));
        let hash = lucide_icon("hash").unwrap();
        assert_eq!(hash.label, "Hash");
        assert!(hash.svg.starts_with("<svg "));
        assert!(hash.svg.contains("stroke=\"currentColor\""));
        for name in ["home", "Hash", "../hash", "<svg>", ""] {
            assert!(lucide_icon(name).is_none(), "{name}");
        }
    }
}
