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
    /// Well-known credential formats and long random-looking tokens.
    Secret,
    /// The value in `password: value`, `api key = value` and the like.
    LabeledSecret,
    /// Runs of mask characters (password dots).
    Masked,
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
            "secret" => Kind::Secret,
            "labeled_secret" => Kind::LabeledSecret,
            "masked" => Kind::Masked,
            _ => return None,
        })
    }
    pub const DEFAULT: [Kind; 5] = [Kind::Email, Kind::Phone, Kind::Card, Kind::Ssn, Kind::Ip];
    /// What Hide secrets looks for: passwords and keys, not personal data.
    pub const SECRETS: [Kind; 3] = [Kind::Secret, Kind::LabeledSecret, Kind::Masked];
}

const MASK_CHARS: &str = "•●*∙·⚫";

/// Labels that introduce a secret value, lowercase.
const LABELS: [&str; 14] = [
    "password",
    "passwd",
    "passcode",
    "pwd",
    "secret",
    "token",
    "api key",
    "api_key",
    "apikey",
    "access key",
    "access_key",
    "private key",
    "كلمة المرور",
    "كلمة السر",
];
const LABELS_EXTRA: [&str; 2] = ["الرمز السري", "كلمة السرّ"];

fn lower(c: &[char]) -> Vec<char> {
    c.iter()
        .map(|ch| ch.to_lowercase().next().unwrap_or(*ch))
        .collect()
}

fn matches_at(hay: &[char], at: usize, needle: &str) -> bool {
    let n: Vec<char> = needle.chars().collect();
    hay.get(at..at + n.len()).is_some_and(|s| s == n.as_slice())
}

/// `(start, end)` of every label occurrence (word-bounded on the left).
fn label_spans(low: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for i in 0..low.len() {
        if i > 0 && low[i - 1].is_alphanumeric() {
            continue;
        }
        for l in LABELS.iter().chain(LABELS_EXTRA.iter()) {
            if matches_at(low, i, l) {
                out.push((i, i + l.chars().count()));
                break;
            }
        }
    }
    out
}

/// The text ends with a secret label (and maybe `:` or `=`): the value is on the next line or to
/// the right, outside this line.
pub fn label_only(text: &str) -> bool {
    let low = lower(&text.chars().collect::<Vec<_>>());
    label_spans(&low).iter().any(|&(_, e)| {
        low[e..]
            .iter()
            .all(|ch| ch.is_whitespace() || ":=\"'：".contains(*ch))
    })
}

fn is_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || "_-+/=.".contains(ch)
}

/// Token ranges (maximal runs of token characters).
fn tokens(c: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if is_token_char(c[i]) {
            let s = i;
            while i < c.len() && is_token_char(c[i]) {
                i += 1;
            }
            out.push((s, i));
        } else {
            i += 1;
        }
    }
    out
}

fn secret_format(tok: &str) -> bool {
    let n = tok.chars().count();
    let body_ok = |from: usize| {
        tok.chars()
            .skip(from)
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    };
    let starts = |p: &str| tok.starts_with(p);
    if (starts("AKIA") || starts("ASIA"))
        && n == 20
        && tok
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
    {
        return true;
    }
    if ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"]
        .iter()
        .any(|p| starts(p))
        && n >= 30
    {
        return body_ok(4);
    }
    if starts("github_pat_") && n >= 30 {
        return body_ok(11);
    }
    if ["sk_live_", "rk_live_", "sk_test_", "rk_test_", "pk_live_"]
        .iter()
        .any(|p| starts(p))
        && n >= 20
    {
        return body_ok(3);
    }
    if starts("sk-") && n >= 20 {
        return body_ok(3);
    }
    if ["xoxb-", "xoxp-", "xoxa-", "xoxs-", "xoxr-"]
        .iter()
        .any(|p| starts(p))
        && n >= 15
    {
        return body_ok(5);
    }
    if starts("AIza") && n == 39 {
        return body_ok(4);
    }
    if starts("eyJ") && tok.matches('.').count() == 2 && n >= 30 {
        return tok.split('.').all(|part| !part.is_empty());
    }
    false
}

/// A long random-looking token: upper, lower and digits, no sentence punctuation.
fn looks_random(tok: &str) -> bool {
    let n = tok.chars().count();
    if n < 24 || tok.contains('.') || tok.starts_with("http") {
        return false;
    }
    let up = tok.chars().any(|c| c.is_ascii_uppercase());
    let lo = tok.chars().any(|c| c.is_ascii_lowercase());
    let digits = tok.chars().filter(char::is_ascii_digit).count();
    let mut distinct: Vec<char> = tok.chars().collect();
    distinct.sort_unstable();
    distinct.dedup();
    up && lo && digits >= 2 && distinct.len() >= 10
}

fn find_secrets(c: &[char], out: &mut Vec<(Kind, usize, usize)>) {
    let toks = tokens(c);
    for (idx, &(s, e)) in toks.iter().enumerate() {
        // Trim trailing '=' padding and punctuation-only edges before the format checks.
        let tok: String = c[s..e].iter().collect();
        let trimmed = tok.trim_end_matches(['.', '=']);
        let t_end = s + trimmed.chars().count();
        if secret_format(trimmed) || looks_random(trimmed) {
            out.push((Kind::Secret, s, t_end.max(s + 1)));
            continue;
        }
        if tok.eq_ignore_ascii_case("bearer") {
            if let Some(&(ns, ne)) = toks.get(idx + 1) {
                let gap_ok = c[e..ns].iter().all(|ch| ch.is_whitespace());
                if gap_ok && ne - ns >= 12 {
                    out.push((Kind::Secret, ns, ne));
                }
            }
        }
    }
    let line: String = c.iter().collect();
    if line.contains("-----BEGIN") && line.contains("PRIVATE KEY") {
        out.push((Kind::Secret, 0, c.len()));
    }
}

fn find_labeled(c: &[char], out: &mut Vec<(Kind, usize, usize)>) {
    let low = lower(c);
    for (_, e) in label_spans(&low) {
        let mut k = e;
        while k < c.len() && (c[k].is_whitespace() || "\"'".contains(c[k])) {
            k += 1;
        }
        if k >= c.len() || !":=：".contains(c[k]) {
            continue;
        }
        k += 1;
        while k < c.len() && (c[k].is_whitespace() || "\"'".contains(c[k])) {
            k += 1;
        }
        let start = k;
        while k < c.len() && !c[k].is_whitespace() && !"\"';,".contains(c[k]) {
            k += 1;
        }
        if k > start {
            out.push((Kind::LabeledSecret, start, k));
        }
    }
}

fn find_masked(c: &[char], out: &mut Vec<(Kind, usize, usize)>) {
    let mut i = 0;
    while i < c.len() {
        if MASK_CHARS.contains(c[i]) {
            let s = i;
            while i < c.len() && MASK_CHARS.contains(c[i]) {
                i += 1;
            }
            if i - s >= 4 {
                out.push((Kind::Masked, s, i));
            }
        } else {
            i += 1;
        }
    }
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
    if want(Kind::Secret) {
        find_secrets(&c, &mut out);
    }
    if want(Kind::LabeledSecret) {
        find_labeled(&c, &mut out);
    }
    if want(Kind::Masked) {
        find_masked(&c, &mut out);
    }
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

    fn found(t: &str) -> Vec<String> {
        spans(t, &Kind::SECRETS)
            .into_iter()
            .map(|(_, s)| s)
            .collect()
    }

    #[test]
    fn well_known_key_formats() {
        for k in [
            "AKIAIOSFODNN7EXAMPLE",
            "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
            "github_pat_11ABCDEFG0abcdefghijklmnopqrstuvwxyz0123456789",
            "sk-proj-abcdefghijklmnopqrstuv1234",
            "sk_live_51H8abcdefghijklmnop",
            "xoxb-123456789012-abcdefghij",
            "AIzaSyA-1234567890abcdefghijklmnopqrstuv",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.dBjftJeZ4CVPmB92K27uhbUJU1p1r",
        ] {
            assert_eq!(found(&format!("key {k} end")), vec![k.to_string()], "{k}");
        }
        assert_eq!(
            found("Authorization: Bearer abcDEF123456xyz"),
            vec!["abcDEF123456xyz".to_string()]
        );
        assert_eq!(found("-----BEGIN RSA PRIVATE KEY-----").len(), 1);
    }

    #[test]
    fn long_random_tokens_but_not_words_or_urls() {
        assert_eq!(
            found("id aB3dE5gH7jK9mN1pQ3sT5vW7yZ9 ok"),
            vec!["aB3dE5gH7jK9mN1pQ3sT5vW7yZ9".to_string()]
        );
        assert!(found("internationalization and configuration").is_empty());
        assert!(found("see https://example.com/aB3dE5gH7jK9mN1pQ3sT5vW7yZ9").is_empty());
        assert!(found("report-2024-final-version.pdf").is_empty());
        assert!(found("order 12345 shipped to Springfield").is_empty());
    }

    #[test]
    fn labeled_values_english_and_arabic() {
        assert_eq!(found("Password: hunter2"), vec!["hunter2".to_string()]);
        assert_eq!(
            found("db_password = \"s3cr3t!\";"),
            vec!["s3cr3t!".to_string()]
        );
        assert_eq!(found("API Key: abc123"), vec!["abc123".to_string()]);
        assert_eq!(found("Server=x;Pwd=p@ss;Port=1"), vec!["p@ss".to_string()]);
        assert_eq!(found("كلمة المرور: 12345678"), vec!["12345678".to_string()]);
        assert!(found("Reset your password today").is_empty());
        assert!(found("tokenizer: fast").is_empty());
    }

    #[test]
    fn masked_runs_and_label_only_lines() {
        assert_eq!(found("pass ●●●●●●●● ok"), vec!["●●●●●●●●".to_string()]);
        assert!(found("a ••• b").is_empty(), "three dots are not a password");
        assert!(label_only("Password:"));
        assert!(label_only("API key "));
        assert!(!label_only("Password: x"));
        assert!(!label_only("Hello"));
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
