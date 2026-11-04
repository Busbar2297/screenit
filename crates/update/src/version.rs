//! `major.minor.patch` versions as used by the release tags (`v0.2.11`).

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// Parse `0.2.11` or `v0.2.11`. Pre-release and build suffixes are refused.
    pub fn parse(text: &str) -> Option<Version> {
        let t = text.trim();
        let t = t
            .strip_prefix('v')
            .or_else(|| t.strip_prefix('V'))
            .unwrap_or(t);
        if t.len() > 32 {
            return None;
        }
        let mut it = t.split('.');
        let major = it.next()?.parse().ok()?;
        let minor = it.next()?.parse().ok()?;
        let patch = it.next()?.parse().ok()?;
        if it.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_orders() {
        let a = Version::parse("v0.2.11").unwrap();
        let b = Version::parse("0.2.9").unwrap();
        assert!(a > b);
        assert!(Version::parse("1.0.0").unwrap() > a);
        assert_eq!(a.to_string(), "0.2.11");
    }

    #[test]
    fn refuses_junk() {
        for bad in [
            "",
            "v",
            "1.2",
            "1.2.3.4",
            "1.2.3-beta",
            "a.b.c",
            "1.2.-3",
            "99999999999.0.0",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }
}
