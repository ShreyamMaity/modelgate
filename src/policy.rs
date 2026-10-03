//! Per-group routing policy: sticky pins, strict single-model groups, timeouts and retries.

use serde_json::{json, Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Sticky,
    Strict,
    Failover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Conversation,
    Day,
    UntilChanged,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    pub mode: Mode,
    pub scope: Scope,
    pub ttl_minutes: u64,
    pub failover_on_timeouts: bool,
    pub retry_same_model: u32,
    pub ttfb_seconds: Option<f64>,
    pub total_timeout_seconds: Option<f64>,
    pub cooldown_seconds: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            mode: Mode::Sticky,
            scope: Scope::Conversation,
            ttl_minutes: 720,
            failover_on_timeouts: true,
            retry_same_model: 0,
            ttfb_seconds: None,
            total_timeout_seconds: None,
            cooldown_seconds: 30,
        }
    }
}

const MODES: &[&str] = &["sticky", "strict", "failover"];
const SCOPES: &[&str] = &["conversation", "day", "until_changed"];
const FAILOVER_ON: &[&str] = &["errors", "errors_and_timeouts"];
const INTS: &[(&str, u64, u64)] = &[
    ("sticky_ttl_minutes", 1, 10_080),
    ("retry_same_model", 0, 5),
    ("cooldown_seconds", 0, 3600),
];
const NUMS: &[(&str, f64, f64)] = &[
    ("ttfb_seconds", 3.0, 300.0),
    ("total_timeout_seconds", 10.0, 1800.0),
];

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Sticky => "sticky",
            Mode::Strict => "strict",
            Mode::Failover => "failover",
        }
    }
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Conversation => "conversation",
            Scope::Day => "day",
            Scope::UntilChanged => "until_changed",
        }
    }
}

impl Policy {
    pub fn from_value(v: Option<&Value>) -> Policy {
        let mut p = Policy::default();
        let Some(o) = v.and_then(Value::as_object) else {
            return p;
        };
        let s = |k: &str| o.get(k).and_then(Value::as_str);
        p.mode = match s("mode") {
            Some("strict") => Mode::Strict,
            Some("failover") => Mode::Failover,
            _ => Mode::Sticky,
        };
        p.scope = match s("sticky_scope") {
            Some("day") => Scope::Day,
            Some("until_changed") => Scope::UntilChanged,
            _ => Scope::Conversation,
        };
        p.failover_on_timeouts = s("failover_on") != Some("errors");
        let int = |k: &str| {
            let (_, lo, hi) = INTS.iter().find(|x| x.0 == k)?;
            o.get(k)
                .and_then(Value::as_u64)
                .filter(|n| (*lo..=*hi).contains(n))
        };
        let num = |k: &str| {
            let (_, lo, hi) = NUMS.iter().find(|x| x.0 == k)?;
            o.get(k)
                .and_then(Value::as_f64)
                .filter(|n| (*lo..=*hi).contains(n))
        };
        if let Some(n) = int("sticky_ttl_minutes") {
            p.ttl_minutes = n;
        }
        if let Some(n) = int("retry_same_model") {
            p.retry_same_model = n as u32;
        }
        if let Some(n) = int("cooldown_seconds") {
            p.cooldown_seconds = n;
        }
        p.ttfb_seconds = num("ttfb_seconds");
        p.total_timeout_seconds = num("total_timeout_seconds");
        p
    }

    pub fn to_json(&self) -> Value {
        json!({
            "mode": self.mode.name(),
            "sticky_scope": self.scope.name(),
            "sticky_ttl_minutes": self.ttl_minutes,
            "failover_on": if self.failover_on_timeouts { "errors_and_timeouts" } else { "errors" },
            "retry_same_model": self.retry_same_model,
            "ttfb_seconds": self.ttfb_seconds,
            "total_timeout_seconds": self.total_timeout_seconds,
            "cooldown_seconds": self.cooldown_seconds,
        })
    }

    pub fn header(&self) -> String {
        format!(
            "mode={}; scope={}; failover_on={}",
            self.mode.name(),
            self.scope.name(),
            if self.failover_on_timeouts {
                "errors_and_timeouts"
            } else {
                "errors"
            }
        )
    }
}

pub fn fields() -> Value {
    let mut m = Map::new();
    m.insert("mode".into(), json!({"type": "enum", "values": MODES}));
    m.insert(
        "sticky_scope".into(),
        json!({"type": "enum", "values": SCOPES}),
    );
    m.insert(
        "failover_on".into(),
        json!({"type": "enum", "values": FAILOVER_ON}),
    );
    for (k, lo, hi) in INTS {
        m.insert(
            (*k).into(),
            json!({"type": "integer", "min": lo, "max": hi}),
        );
    }
    for (k, lo, hi) in NUMS {
        m.insert(
            (*k).into(),
            json!({"type": "number", "min": lo, "max": hi, "nullable": true}),
        );
    }
    Value::Object(m)
}

/// Checks one group's policy object. Returns the cleaned object or (field, message).
pub fn validate(v: &Value) -> Result<Map<String, Value>, (String, String)> {
    let o = v
        .as_object()
        .ok_or_else(|| ("policy".to_owned(), "policy must be an object".to_owned()))?;
    let mut out = Map::new();
    for (k, val) in o {
        let enumv = match k.as_str() {
            "mode" => Some(MODES),
            "sticky_scope" => Some(SCOPES),
            "failover_on" => Some(FAILOVER_ON),
            _ => None,
        };
        if let Some(allowed) = enumv {
            let s = val
                .as_str()
                .filter(|s| allowed.contains(s))
                .ok_or_else(|| {
                    (
                        k.clone(),
                        format!("{k} must be one of {}", allowed.join(", ")),
                    )
                })?;
            out.insert(k.clone(), json!(s));
        } else if let Some((_, lo, hi)) = INTS.iter().find(|x| x.0 == k) {
            let n = val
                .as_u64()
                .or_else(|| {
                    val.as_f64()
                        .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                        .map(|f| f as u64)
                })
                .filter(|n| (*lo..=*hi).contains(n))
                .ok_or_else(|| {
                    (
                        k.clone(),
                        format!("{k} must be a whole number between {lo} and {hi}"),
                    )
                })?;
            out.insert(k.clone(), json!(n));
        } else if let Some((_, lo, hi)) = NUMS.iter().find(|x| x.0 == k) {
            if val.is_null() {
                continue;
            }
            let n = val
                .as_f64()
                .filter(|n| (*lo..=*hi).contains(n))
                .ok_or_else(|| {
                    (
                        k.clone(),
                        format!("{k} must be a number between {lo} and {hi}, or null"),
                    )
                })?;
            out.insert(k.clone(), json!(n));
        } else {
            return Err((k.clone(), format!("unknown policy field {k}")));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_parse() {
        let p = Policy::from_value(None);
        assert_eq!(p, Policy::default());
        assert_eq!(p.mode, Mode::Sticky);
        let p = Policy::from_value(Some(&json!({"mode": "strict", "sticky_scope": "day",
            "failover_on": "errors", "retry_same_model": 2, "ttfb_seconds": 45, "total_timeout_seconds": 600,
            "cooldown_seconds": 5, "sticky_ttl_minutes": 30})));
        assert_eq!(p.mode, Mode::Strict);
        assert_eq!(p.scope, Scope::Day);
        assert!(!p.failover_on_timeouts);
        assert_eq!(p.retry_same_model, 2);
        assert_eq!(p.ttfb_seconds, Some(45.0));
        assert_eq!(p.total_timeout_seconds, Some(600.0));
        assert_eq!(p.cooldown_seconds, 5);
        assert_eq!(p.ttl_minutes, 30);
        let p = Policy::from_value(Some(&json!({"mode": "weird", "retry_same_model": 99})));
        assert_eq!(p.mode, Mode::Sticky);
        assert_eq!(p.retry_same_model, 0);
        let round = Policy::from_value(Some(&p.to_json()));
        assert_eq!(round, p);
    }

    #[test]
    fn validation() {
        assert!(validate(
            &json!({"mode": "sticky", "ttfb_seconds": null, "retry_same_model": 1.0})
        )
        .is_ok());
        assert_eq!(validate(&json!({"mode": "x"})).unwrap_err().0, "mode");
        assert_eq!(
            validate(&json!({"ttfb_seconds": 1})).unwrap_err().0,
            "ttfb_seconds"
        );
        assert_eq!(
            validate(&json!({"retry_same_model": 1.5})).unwrap_err().0,
            "retry_same_model"
        );
        assert_eq!(validate(&json!({"nope": 1})).unwrap_err().0, "nope");
        assert!(validate(&json!([])).is_err());
    }
}
