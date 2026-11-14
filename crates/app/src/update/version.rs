//! Strict `X.Y.Z` versions, as used by the workspace `Cargo.toml` and the `vX.Y.Z` release tags.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// Strict `X.Y.Z` or `vX.Y.Z`: three ASCII-digit parts, no sign, no leading zeros except "0",
    /// no suffix or spaces, no u32 overflow.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.strip_prefix('v').unwrap_or(s);
        let mut parts = s.split('.');
        let major = part(parts.next()?)?;
        let minor = part(parts.next()?)?;
        let patch = part(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    /// A release tag: must be `vX.Y.Z`.
    pub fn parse_tag(tag: &str) -> Option<Self> {
        tag.strip_prefix('v').and(Self::parse(tag))
    }

    /// From env!("CARGO_PKG_VERSION").
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("the workspace version is strict X.Y.Z")
    }
}

/// One version part: ASCII digits only, no leading zero unless it is "0", fits in a u32.
fn part(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return None;
    }
    s.parse().ok()
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u32, minor: u32, patch: u32) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    #[test]
    fn parses_strict_versions() {
        assert_eq!(Version::parse("1.2.3"), Some(v(1, 2, 3)));
        assert_eq!(Version::parse("v1.2.3"), Some(v(1, 2, 3)));
        assert_eq!(Version::parse("0.0.0"), Some(v(0, 0, 0)));
        assert_eq!(Version::parse("10.20.30"), Some(v(10, 20, 30)));
    }

    #[test]
    fn rejects_non_strict() {
        for s in [
            "",
            "1.2",
            "1.2.3.4",
            "1.2.3-dev.1",
            " 1.2.3",
            "1.2.3 ",
            "+1.2.3",
            "1.02.3",
            "a.b.c",
            "1..3",
            "V1.2.3",
            "99999999999.0.0",
        ] {
            assert_eq!(Version::parse(s), None, "{s:?}");
        }
    }

    #[test]
    fn tag_needs_v() {
        assert_eq!(Version::parse_tag("v1.1.0"), Some(v(1, 1, 0)));
        assert_eq!(Version::parse_tag("1.1.0"), None);
        assert_eq!(Version::parse_tag("base-models-1"), None);
    }

    #[test]
    fn orders_numerically() {
        assert!(v(1, 0, 10) > v(1, 0, 9));
        assert!(v(2, 0, 0) > v(1, 99, 99));
    }

    #[test]
    fn displays_without_v() {
        let parsed = Version::parse("v1.2.3").unwrap();
        assert_eq!(parsed.to_string(), "1.2.3");
        assert_eq!(Version::parse(&parsed.to_string()), Some(parsed));
    }

    #[test]
    fn current_matches_cargo() {
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }
}
