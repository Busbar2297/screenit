//! Sensitive-text detection for Smart Redact. Hand-written scanners (no regex dependency); each
//! returns character spans so callers can map them onto OCR line geometry.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Email,
    Phone,
    Card,
    Ssn,
    Ip,
    Url,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "email" => Kind::Email,
            "phone" => Kind::Phone,
            "card" => Kind::Card,
            "ssn" => Kind::Ssn,
            "ip" => Kind::Ip,
            "url" => Kind::Url,
            _ => return None,
        })
    }
    pub const DEFAULT: [Kind; 5] = [Kind::Email, Kind::Phone, Kind::Card, Kind::Ssn, Kind::Ip];
}

fn luhn(digits: &[u32]) -> bool {
    let mut sum = 0;
    for (i, d) in digits.iter().rev().enumerate() {
        let mut v = *d;
        if i % 2 == 1 {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
    }
    sum % 10 == 0
}

/// Find sensitive spans `(kind, start_char, end_char)` in `text`.
pub fn find(text: &str, kinds: &[Kind]) -> Vec<(Kind, usize, usize)> {
    let c: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let want = |k: Kind| kinds.contains(&k);
    if want(Kind::Email) {
        for (i, ch) in c.iter().enumerate() {
            if *ch != '@' {
                continue;
            }
            let mut a = i;
            while a > 0 && (c[a - 1].is_ascii_alphanumeric() || "._%+-".contains(c[a - 1])) {
                a -= 1;
            }
            let mut b = i + 1;
            while b < c.len() && (c[b].is_ascii_alphanumeric() || ".-".contains(c[b])) {
                b += 1;
            }
            while b > i + 1 && c[b - 1] == '.' {
                b -= 1;
            }
            let domain: String = c[i + 1..b].iter().collect();
            if a < i && domain.contains('.') && !domain.starts_with('.') {
                out.push((Kind::Email, a, b));
            }
        }
    }
    if want(Kind::Url) {
        let s: String = c.iter().collect();
        for pre in ["http://", "https://", "www."] {
            let mut from = 0;
            while let Some(p) = s[from..].find(pre) {
                let start_b = from + p;
                let end_b = s[start_b..]
                    .find(char::is_whitespace)
                    .map_or(s.len(), |e| start_b + e);
                let a = s[..start_b].chars().count();
                let b = s[..end_b].chars().count();
                if !out
                    .iter()
                    .any(|(k, x, y)| *k == Kind::Url && a >= *x && a < *y)
                {
                    out.push((Kind::Url, a, b));
                }
                from = end_b.max(start_b + 1);
            }
        }
    }
    // Numeric runs: digits with optional separators.
    let sep = |ch: char| " -().+".contains(ch);
    let mut i = 0;
    while i < c.len() {
        if !c[i].is_ascii_digit() && !(c[i] == '+' || c[i] == '(') {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        let mut j = i;
        while j < c.len() && (c[j].is_ascii_digit() || sep(c[j])) {
            if c[j].is_ascii_digit() {
                end = j + 1;
            }
            j += 1;
        }
        let run: Vec<char> = c[start..end].to_vec();
        let digits: Vec<u32> = run.iter().filter_map(|x| x.to_digit(10)).collect();
        let only_sp_dash = run
            .iter()
            .all(|x| x.is_ascii_digit() || *x == ' ' || *x == '-');
        let run_s: String = run.iter().collect();
        let dotted = run_s.split('.').count() == 4
            && run_s
                .split('.')
                .all(|o| !o.is_empty() && o.len() <= 3 && o.parse::<u32>().is_ok_and(|v| v <= 255));
        if dotted {
            if want(Kind::Ip) {
                out.push((Kind::Ip, start, end));
            }
        } else if want(Kind::Ssn)
            && run.len() == 11
            && only_sp_dash
            && run_s.split('-').map(str::len).collect::<Vec<_>>() == [3, 2, 4]
        {
            out.push((Kind::Ssn, start, end));
        } else if want(Kind::Card)
            && (13..=19).contains(&digits.len())
            && only_sp_dash
            && luhn(&digits)
        {
            out.push((Kind::Card, start, end));
        } else if want(Kind::Phone) && (10..=15).contains(&digits.len()) && !run_s.contains("  ") {
            out.push((Kind::Phone, start, end));
        }
        i = j.max(start + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(t: &str, k: &[Kind]) -> Vec<(Kind, String)> {
        let c: Vec<char> = t.chars().collect();
        find(t, k)
            .into_iter()
            .map(|(k, a, b)| (k, c[a..b].iter().collect()))
            .collect()
    }

    #[test]
    fn finds_email_and_ignores_plain_at() {
        assert_eq!(
            spans("mail bob.s@example.co.uk now", &[Kind::Email]),
            vec![(Kind::Email, "bob.s@example.co.uk".into())]
        );
        assert!(spans("meet @ noon", &[Kind::Email]).is_empty());
        assert!(spans("a@b", &[Kind::Email]).is_empty());
    }

    #[test]
    fn card_needs_luhn_phone_ssn_ip() {
        assert_eq!(
            spans("pay 4111 1111 1111 1111 ok", &Kind::DEFAULT)[0].0,
            Kind::Card
        );
        assert!(
            spans("4111 1111 1111 1112", &[Kind::Card]).is_empty(),
            "bad checksum"
        );
        assert_eq!(
            spans("ssn 123-45-6789", &Kind::DEFAULT)[0],
            (Kind::Ssn, "123-45-6789".into())
        );
        assert_eq!(
            spans("call (555) 123-4567", &[Kind::Phone])[0].0,
            Kind::Phone
        );
        assert_eq!(
            spans("host 192.168.1.20 up", &Kind::DEFAULT)[0],
            (Kind::Ip, "192.168.1.20".into())
        );
        assert!(spans("version 1.2.3.999", &[Kind::Ip]).is_empty());
        assert!(spans("order 12345", &Kind::DEFAULT).is_empty());
    }

    #[test]
    fn urls_only_when_asked() {
        assert!(spans("see https://x.io/a?b=1 now", &Kind::DEFAULT).is_empty());
        assert_eq!(
            spans("see https://x.io/a?b=1 now", &[Kind::Url]),
            vec![(Kind::Url, "https://x.io/a?b=1".into())]
        );
    }
}
