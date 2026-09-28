use regex::bytes::{Captures, Regex, RegexBuilder, RegexSet, RegexSetBuilder};
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: &'static str,
}

#[derive(Clone, Copy)]
enum Check {
    None,
    Password,
    Token,
    Card,
    Aadhaar,
    Bank,
    Phone,
    Intl,
    Pin,
}

struct Rule {
    kind: &'static str,
    pattern: &'static str,
    group: usize,
    check: Check,
}

const RULES: &[Rule] = &[
    Rule {
        kind: "PRIVATE_KEY",
        pattern: r"-----BEGIN[A-Z ]{0,20}PRIVATE KEY-----(?s:.)*?-----END[A-Z ]{0,20}PRIVATE KEY-----",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "JWT",
        pattern: r"\beyJ[A-Za-z0-9_-]{5,}\.eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{10,}",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "PASSWORD",
        pattern: r#"\b[a-zA-Z][a-zA-Z0-9+.-]{1,15}://[^\s:/@"'<>]{1,64}:([^\s@/"'<>]{1,128})@"#,
        group: 1,
        check: Check::None,
    },
    Rule {
        kind: "ANTHROPIC_KEY",
        pattern: r"\bsk-ant-[A-Za-z0-9_-]{20,}",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "API_KEY",
        pattern: r"\bsk-[A-Za-z0-9_-]{20,}",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "GITHUB_TOKEN",
        pattern: r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{40,})",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "AWS_KEY",
        pattern: r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "GOOGLE_API_KEY",
        pattern: r"\bAIza[0-9A-Za-z_-]{35}",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "SLACK_TOKEN",
        pattern: r"\bxox[abposr]-[A-Za-z0-9-]{10,}",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "STRIPE_KEY",
        pattern: r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "API_KEY",
        pattern: r"\b(?:hf_[A-Za-z0-9]{30,}|glpat-[A-Za-z0-9_-]{20,}|gsk_[A-Za-z0-9]{40,}|npm_[A-Za-z0-9]{36}|tskey-[a-z]+-[A-Za-z0-9-]{10,}|r8_[A-Za-z0-9]{30,}|xai-[A-Za-z0-9]{40,}|nvapi-[A-Za-z0-9_-]{40,}|AGE-SECRET-KEY-1[0-9A-Z]{58}|[0-9]{8,10}:AA[A-Za-z0-9_-]{33})",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "PASSWORD",
        pattern: r#"(?i)\b(?:password|passwd|passphrase|pwd|passcode)["']?\s*(?:([:=]|=>)|\b(is)\b)\s*["']?([^\s"',;]{3,128})"#,
        group: 3,
        check: Check::Password,
    },
    Rule {
        kind: "PIN",
        pattern: r"(?i)\b(?:upi\s*pin|atm\s*pin|card\s*pin|m-?pin|cvv2?|cvc)\b\s*(?:[:=#-]|\bis\b)?\s*([0-9]{3,6})\b",
        group: 1,
        check: Check::Pin,
    },
    Rule {
        kind: "TOKEN",
        pattern: r#"(?i)\b(?:api[_ -]?key|apikey|secret[_ -]?key|client[_ -]?secret|access[_ -]?token|auth[_ -]?token|refresh[_ -]?token|private[_ -]?token|token|secret)["']?\s*(?:[:=]|=>)\s*["']?([A-Za-z0-9_\-./+=~]{12,256})"#,
        group: 1,
        check: Check::Token,
    },
    Rule {
        kind: "TOKEN",
        pattern: r"\b[Bb]earer\s+([A-Za-z0-9_\-.~+/]{16,}=*)",
        group: 1,
        check: Check::Token,
    },
    Rule {
        kind: "EMAIL",
        pattern: r"\b[A-Za-z0-9._%+-]{1,64}@[A-Za-z0-9-]{1,63}(?:\.[A-Za-z0-9-]{1,63})*\.[A-Za-z]{2,24}\b",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "UPI",
        pattern: r"(?i)\b[a-z0-9._-]{2,64}@(?:okaxis|oksbi|okhdfcbank|okicici|ybl|ibl|axl|paytm|upi|apl|yapl|rapl|ptyes|ptaxis|pthdfc|ptsbi|waicici|wahdfcbank|waaxis|wasbi|fam|freecharge|kotak|icici|hdfcbank|sbi|axisbank|slice|superyes|naviaxis|jupiteraxis|idfcbank|airtel|jio)\b",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "BANK_ACCOUNT",
        pattern: r"(?i)\b(?:a/c|acct|account|acc)\.?\s*(?:no|number|num|#)?\.?\s*[:#-]?\s*([0-9][0-9 -]{7,24}[0-9])\b",
        group: 1,
        check: Check::Bank,
    },
    Rule {
        kind: "CARD",
        pattern: r"\b[0-9](?:[ -]?[0-9]){12,18}\b",
        group: 0,
        check: Check::Card,
    },
    Rule {
        kind: "AADHAAR",
        pattern: r"\b[2-9][0-9]{3}[ -]?[0-9]{4}[ -]?[0-9]{4}\b",
        group: 0,
        check: Check::Aadhaar,
    },
    Rule {
        kind: "PAN",
        pattern: r"\b[A-Z]{3}[ABCFGHLJPT][A-Z][0-9]{4}[A-Z]\b",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "IFSC",
        pattern: r"\b[A-Z]{4}0[A-Z0-9]{6}\b",
        group: 0,
        check: Check::None,
    },
    Rule {
        kind: "PHONE",
        pattern: r"(?:\+91[\s-]?|\b91[\s-]|\b0|\b)[6-9][0-9]{4}[\s-]?[0-9]{5}\b",
        group: 0,
        check: Check::Phone,
    },
    Rule {
        kind: "PHONE",
        pattern: r"\+[1-9][0-9]{0,2}[\s-]?\(?[0-9]{1,4}\)?(?:[\s-]?[0-9]{2,5}){1,4}\b",
        group: 0,
        check: Check::Intl,
    },
];

pub const KINDS: &[&str] = &[
    "PRIVATE_KEY",
    "JWT",
    "PASSWORD",
    "ANTHROPIC_KEY",
    "API_KEY",
    "GITHUB_TOKEN",
    "AWS_KEY",
    "GOOGLE_API_KEY",
    "SLACK_TOKEN",
    "STRIPE_KEY",
    "PIN",
    "TOKEN",
    "EMAIL",
    "UPI",
    "BANK_ACCOUNT",
    "CARD",
    "AADHAAR",
    "PAN",
    "IFSC",
    "PHONE",
    "ADDRESS",
];

struct Compiled {
    set: RegexSet,
    res: Vec<Regex>,
}

fn compiled() -> &'static Compiled {
    static C: OnceLock<Compiled> = OnceLock::new();
    C.get_or_init(|| Compiled {
        set: RegexSetBuilder::new(RULES.iter().map(|r| r.pattern))
            .unicode(false)
            .dfa_size_limit(256 << 10)
            .build()
            .expect("pii rules"),
        res: RULES
            .iter()
            .map(|r| {
                RegexBuilder::new(r.pattern)
                    .unicode(false)
                    .dfa_size_limit(16 << 10)
                    .build()
                    .expect("pii rule")
            })
            .collect(),
    })
}

pub fn warm() {
    let _ = compiled();
}

pub fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

pub fn luhn(d: &str) -> bool {
    let mut sum = 0u32;
    for (i, c) in d.bytes().rev().enumerate() {
        let mut n = u32::from(c - b'0');
        if i % 2 == 1 {
            n *= 2;
            if n > 9 {
                n -= 9;
            }
        }
        sum += n;
    }
    !d.is_empty() && sum.is_multiple_of(10)
}

const VD: [[u8; 10]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
    [1, 2, 3, 4, 0, 6, 7, 8, 9, 5],
    [2, 3, 4, 0, 1, 7, 8, 9, 5, 6],
    [3, 4, 0, 1, 2, 8, 9, 5, 6, 7],
    [4, 0, 1, 2, 3, 9, 5, 6, 7, 8],
    [5, 9, 8, 7, 6, 0, 4, 3, 2, 1],
    [6, 5, 9, 8, 7, 1, 0, 4, 3, 2],
    [7, 6, 5, 9, 8, 2, 1, 0, 4, 3],
    [8, 7, 6, 5, 9, 3, 2, 1, 0, 4],
    [9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
];
const VP: [[u8; 10]; 8] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
    [1, 5, 7, 6, 2, 8, 3, 0, 9, 4],
    [5, 8, 0, 3, 7, 9, 6, 1, 4, 2],
    [8, 9, 1, 6, 0, 4, 3, 5, 2, 7],
    [9, 4, 5, 3, 1, 2, 6, 8, 7, 0],
    [4, 2, 8, 6, 5, 7, 3, 9, 0, 1],
    [2, 7, 9, 3, 8, 0, 6, 4, 1, 5],
    [7, 0, 4, 6, 9, 1, 3, 2, 5, 8],
];

pub fn verhoeff(d: &str) -> bool {
    let mut c = 0usize;
    for (i, b) in d.bytes().rev().enumerate() {
        c = VD[c][VP[i % 8][usize::from(b - b'0')] as usize] as usize;
    }
    !d.is_empty() && c == 0
}

fn all_same(d: &str) -> bool {
    d.bytes().all(|b| Some(b) == d.bytes().next())
}

fn valid(check: Check, caps: &Captures, v: &str) -> bool {
    if v.starts_with('<') {
        return false;
    }
    match check {
        Check::None => true,
        Check::Password => {
            if caps.get(2).is_some() {
                v.chars().any(|c| !c.is_alphabetic())
            } else {
                true
            }
        }
        Check::Token => {
            v.chars().any(|c| c.is_ascii_digit())
                && v.chars().any(|c| c.is_ascii_alphabetic())
                && !v.contains("://")
        }
        Check::Pin => true,
        Check::Card => {
            let d = digits(v);
            (13..=19).contains(&d.len())
                && matches!(d.as_bytes()[0], b'2'..=b'6' | b'8')
                && !all_same(&d)
                && luhn(&d)
        }
        Check::Aadhaar => {
            let d = digits(v);
            d.len() == 12 && !all_same(&d) && verhoeff(&d)
        }
        Check::Bank => (9..=18).contains(&digits(v).len()),
        Check::Phone => {
            let d = digits(v);
            let core = &d[d.len().saturating_sub(10)..];
            core.len() == 10 && !all_same(core)
        }
        Check::Intl => (8..=15).contains(&digits(v).len()),
    }
}

pub fn normalize(kind: &str, raw: &str) -> String {
    match kind {
        "CARD" | "AADHAAR" | "BANK_ACCOUNT" | "PIN" => digits(raw),
        "PHONE" => {
            let d = digits(raw);
            if raw.trim_start().starts_with('+') && !raw.trim_start().starts_with("+91") {
                d
            } else {
                d[d.len().saturating_sub(10)..].to_owned()
            }
        }
        "EMAIL" | "UPI" => raw.to_ascii_lowercase(),
        "PERSON" | "NAME" | "ORG" | "LOCATION" | "ADDRESS" => fold_name(raw),
        _ => raw.to_owned(),
    }
}

fn fold(c: char) -> char {
    match c {
        '\u{e0}'..='\u{e5}' | '\u{101}' | '\u{103}' | '\u{105}' => 'a',
        '\u{e7}' | '\u{107}' | '\u{10d}' => 'c',
        '\u{e8}'..='\u{eb}' | '\u{113}' | '\u{117}' | '\u{119}' => 'e',
        '\u{ec}'..='\u{ef}' | '\u{12b}' | '\u{12f}' => 'i',
        '\u{f1}' | '\u{144}' | '\u{1e45}' | '\u{1e47}' => 'n',
        '\u{f2}'..='\u{f6}' | '\u{f8}' | '\u{14d}' | '\u{151}' => 'o',
        '\u{f9}'..='\u{fc}' | '\u{16b}' | '\u{16f}' | '\u{171}' => 'u',
        '\u{fd}' | '\u{ff}' => 'y',
        '\u{15b}' | '\u{161}' | '\u{15f}' | '\u{1e63}' => 's',
        '\u{1e6d}' => 't',
        '\u{1e0d}' => 'd',
        '\u{1e5b}' | '\u{1e5d}' => 'r',
        '\u{1e25}' => 'h',
        '\u{1e43}' => 'm',
        _ => c,
    }
}

pub fn fold_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut gap = false;
    for c in raw.chars().flat_map(char::to_lowercase).map(fold) {
        if c.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

fn overlaps(spans: &[Span], s: usize, e: usize) -> bool {
    spans.iter().any(|x| s < x.end && x.start < e)
}

pub fn detect_into(text: &str, allow: &dyn Fn(&str) -> bool, spans: &mut Vec<Span>) {
    let c = compiled();
    let hits = c.set.matches(text.as_bytes());
    if !hits.matched_any() {
        if allow("ADDRESS") {
            addresses_into(text, spans);
        }
        return;
    }
    for i in hits.iter() {
        let rule = &RULES[i];
        if !allow(rule.kind) {
            continue;
        }
        for caps in c.res[i].captures_iter(text.as_bytes()) {
            let Some(m) = caps.get(rule.group) else {
                continue;
            };
            let (mut s, e) = (m.start(), m.end());
            if !text.is_char_boundary(s) || !text.is_char_boundary(e) {
                continue;
            }
            if matches!(rule.check, Check::Phone) {
                let lead = text[s..e].len() - text[s..e].trim_start().len();
                s += lead;
            }
            if s >= e || !valid(rule.check, &caps, &text[s..e]) || overlaps(spans, s, e) {
                continue;
            }
            spans.push(Span {
                start: s,
                end: e,
                kind: rule.kind,
            });
        }
    }
    if allow("ADDRESS") {
        addresses_into(text, spans);
    }
}

fn addresses_into(text: &str, spans: &mut Vec<Span>) {
    if !text.bytes().any(|b| b.is_ascii_digit()) {
        return;
    }
    for (s, e) in super::address::find(text) {
        if !overlaps(spans, s, e) {
            spans.push(Span {
                start: s,
                end: e,
                kind: "ADDRESS",
            });
        }
    }
}

pub fn detect(text: &str) -> Vec<Span> {
    let mut v = Vec::new();
    detect_into(text, &|_| true, &mut v);
    v.sort_by_key(|s| s.start);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(t: &str) -> Vec<(&'static str, String)> {
        detect(t)
            .into_iter()
            .map(|s| (s.kind, t[s.start..s.end].to_owned()))
            .collect()
    }

    fn one(t: &str, kind: &str, val: &str) {
        let k = kinds(t);
        assert!(
            k.iter().any(|(a, b)| *a == kind && b == val),
            "{t:?} -> {k:?}, wanted {kind} {val:?}"
        );
    }

    fn none(t: &str) {
        assert!(kinds(t).is_empty(), "{t:?} -> {:?}", kinds(t));
    }

    #[test]
    fn luhn_and_verhoeff() {
        assert!(luhn("4111111111111111"));
        assert!(!luhn("4111111111111112"));
        assert!(verhoeff("234123412346"));
        assert!(!verhoeff("234123412345"));
    }

    #[test]
    fn cards() {
        one(
            "pay with 4111 1111 1111 1111 now",
            "CARD",
            "4111 1111 1111 1111",
        );
        one("card 5500-0000-0000-0004", "CARD", "5500-0000-0000-0004");
        one("amex 378282246310005.", "CARD", "378282246310005");
        none("order 4111 1111 1111 1112 failed");
        none("id 1234567890123456");
    }

    #[test]
    fn aadhaar() {
        one("aadhaar 2341 2341 2346", "AADHAAR", "2341 2341 2346");
        one("uid=234123412346", "AADHAAR", "234123412346");
        none("ref 2341 2341 2345");
    }

    #[test]
    fn pan_ifsc_upi() {
        one("PAN: ABCPE1234F", "PAN", "ABCPE1234F");
        none("code ABCXE1234F");
        one("IFSC HDFC0001234 branch", "IFSC", "HDFC0001234");
        one("send to jane.doe@okaxis please", "UPI", "jane.doe@okaxis");
    }

    #[test]
    fn emails_phones() {
        one(
            "mail jane.doe+x@example.co.in ok",
            "EMAIL",
            "jane.doe+x@example.co.in",
        );
        one("call +91 98765 43210", "PHONE", "+91 98765 43210");
        one("call +91-9876543210 now", "PHONE", "+91-9876543210");
        one("mobile 9876543210", "PHONE", "9876543210");
        one("us +1 415 555 2671", "PHONE", "+1 415 555 2671");
        none("unix time 1727481600 and year 2026");
    }

    #[test]
    fn bank_and_pin() {
        one("A/c No: 50100123456789", "BANK_ACCOUNT", "50100123456789");
        one(
            "account number 1234 5678 9012",
            "BANK_ACCOUNT",
            "1234 5678 9012",
        );
        one("my UPI PIN is 482913", "PIN", "482913");
        one("cvv: 123", "PIN", "123");
    }

    #[test]
    fn keys_and_tokens() {
        one(
            "key sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWx",
            "ANTHROPIC_KEY",
            "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWx",
        );
        one(
            "OPENAI_API_KEY=sk-proj-AbCdEfGhIjKlMnOpQrStUvWx12",
            "API_KEY",
            "sk-proj-AbCdEfGhIjKlMnOpQrStUvWx12",
        );
        one(
            "maps AIzaSyA1234567890abcdefghijklmnopqrstuv",
            "GOOGLE_API_KEY",
            "AIzaSyA1234567890abcdefghijklmnopqrstuv",
        );
        one(
            "gh ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "GITHUB_TOKEN",
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
        );
        one(
            "aws AKIAIOSFODNN7EXAMPLE x",
            "AWS_KEY",
            "AKIAIOSFODNN7EXAMPLE",
        );
        one(
            "slack xoxb-123456789012-abcdefghij",
            "SLACK_TOKEN",
            "xoxb-123456789012-abcdefghij",
        );
        one(
            "Authorization: Bearer abcDEF123456ghiJKL789",
            "TOKEN",
            "abcDEF123456ghiJKL789",
        );
        one(
            "api_key = \"q8Zr2LmN4vB7xT1k\"",
            "TOKEN",
            "q8Zr2LmN4vB7xT1k",
        );
        none("the api key is required and tokens: many");
    }

    #[test]
    fn jwt_and_private_key() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        one(&format!("token {jwt} end"), "JWT", jwt);
        let pk = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----";
        one(&format!("key:\n{pk}\nthanks"), "PRIVATE_KEY", pk);
        let esc = "-----BEGIN RSA PRIVATE KEY-----\\nMIIEow\\n-----END RSA PRIVATE KEY-----";
        one(esc, "PRIVATE_KEY", esc);
    }

    #[test]
    fn passwords() {
        one("password: hunter2!", "PASSWORD", "hunter2!");
        one("\"password\": \"s3cret-pw\"", "PASSWORD", "s3cret-pw");
        one("my password is Tr0ub4dor", "PASSWORD", "Tr0ub4dor");
        one("PDF pwd=ABCD1990", "PASSWORD", "ABCD1990");
        one(
            "postgres://admin:pa55word@db.internal:5432/app",
            "PASSWORD",
            "pa55word",
        );
        none("the password is required");
        none("password: <PASSWORD_A>");
    }
}
