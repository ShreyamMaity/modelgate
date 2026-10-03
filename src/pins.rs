//! Session pins for sticky groups and the failover event log, persisted to a small JSON file.

use crate::policy::Scope;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

const MAX_PINS: usize = 5000;
const MAX_EVENTS: usize = 200;
const IST: u64 = 19_800;
pub const GROUP_WIDE: &str = "*";

#[derive(Clone, Debug)]
pub struct Pin {
    pub group: String,
    pub session: String,
    pub model: String,
    pub scope: String,
    pub source: String,
    pub sig: String,
    pub set_at: u64,
    pub last_used: u64,
    pub ttl_minutes: u64,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub time: u64,
    pub group: String,
    pub session: String,
    pub from: String,
    pub to: Option<String>,
    pub reason: String,
}

#[derive(Default)]
struct Inner {
    pins: HashMap<(String, String), Pin>,
    events: VecDeque<Event>,
    dirty: bool,
}

pub struct Pins {
    path: Option<PathBuf>,
    inner: Mutex<Inner>,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn next_ist_midnight(t: u64) -> u64 {
    ((t + IST) / 86_400 + 1) * 86_400 - IST
}

pub fn signature(list: &[String]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in list.join("\n").bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

impl Pin {
    pub fn expires_at(&self) -> Option<u64> {
        match self.scope.as_str() {
            "conversation" => Some(self.last_used + self.ttl_minutes * 60),
            "day" => Some(next_ist_midnight(self.set_at)),
            _ => None,
        }
    }

    fn json(&self) -> Value {
        json!({"group": self.group, "session": self.session, "model": self.model, "scope": self.scope,
               "source": self.source, "set_at": self.set_at, "last_used": self.last_used,
               "expires_at": self.expires_at()})
    }

    fn parse(v: &Value) -> Option<Pin> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        let n = |k: &str| v.get(k).and_then(Value::as_u64);
        Some(Pin {
            group: s("group")?,
            session: s("session")?,
            model: s("model")?,
            scope: s("scope")?,
            source: s("source").unwrap_or_default(),
            sig: s("sig").unwrap_or_default(),
            set_at: n("set_at")?,
            last_used: n("last_used")?,
            ttl_minutes: n("ttl_minutes").unwrap_or(720),
        })
    }
}

impl Event {
    fn json(&self) -> Value {
        json!({"time": self.time, "group": self.group, "session": self.session, "from": self.from,
               "to": self.to, "reason": self.reason})
    }

    fn parse(v: &Value) -> Option<Event> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        Some(Event {
            time: v.get("time").and_then(Value::as_u64)?,
            group: s("group")?,
            session: s("session").unwrap_or_default(),
            from: s("from").unwrap_or_default(),
            to: s("to"),
            reason: s("reason").unwrap_or_default(),
        })
    }
}

impl Pins {
    pub fn load(path: Option<PathBuf>) -> Pins {
        let mut inner = Inner::default();
        if let Some(v) = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        {
            let t = now();
            for p in v["pins"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Pin::parse)
            {
                if p.expires_at().is_none_or(|e| e > t) {
                    inner.pins.insert((p.group.clone(), p.session.clone()), p);
                }
            }
            for e in v["events"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Event::parse)
            {
                inner.events.push_back(e);
            }
            inner.events.truncate(MAX_EVENTS);
        }
        Pins {
            path,
            inner: Mutex::new(inner),
        }
    }

    pub fn key(scope: Scope, session: Option<&str>) -> Option<String> {
        match scope {
            Scope::Conversation => session.map(str::to_owned),
            _ => Some(GROUP_WIDE.to_owned()),
        }
    }

    /// The live pin for (group, key), dropped if it expired, its model left the group, or (for
    /// until_changed) the group's list changed.
    pub fn get(&self, group: &str, key: &str, list: &[String], scope: Scope) -> Option<Pin> {
        let mut g = self.inner.lock().unwrap();
        let k = (group.to_owned(), key.to_owned());
        let p = g.pins.get(&k)?.clone();
        let t = now();
        let stale = p.expires_at().is_some_and(|e| e <= t)
            || !list.contains(&p.model)
            || p.scope != scope.name()
            || (scope == Scope::UntilChanged && p.sig != signature(list));
        if stale {
            g.pins.remove(&k);
            g.dirty = true;
            return None;
        }
        Some(p)
    }

    pub fn touch(&self, group: &str, key: &str) {
        let mut g = self.inner.lock().unwrap();
        if let Some(p) = g.pins.get_mut(&(group.to_owned(), key.to_owned())) {
            p.last_used = now();
            g.dirty = true;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set(
        &self,
        group: &str,
        key: &str,
        model: &str,
        scope: Scope,
        ttl_minutes: u64,
        source: &str,
        list: &[String],
    ) {
        let t = now();
        {
            let mut g = self.inner.lock().unwrap();
            if g.pins.len() >= MAX_PINS {
                if let Some(oldest) = g
                    .pins
                    .iter()
                    .min_by_key(|(_, p)| p.last_used)
                    .map(|(k, _)| k.clone())
                {
                    g.pins.remove(&oldest);
                }
            }
            g.pins.insert(
                (group.to_owned(), key.to_owned()),
                Pin {
                    group: group.to_owned(),
                    session: key.to_owned(),
                    model: model.to_owned(),
                    scope: scope.name().to_owned(),
                    source: source.to_owned(),
                    sig: signature(list),
                    set_at: t,
                    last_used: t,
                    ttl_minutes,
                },
            );
            g.dirty = true;
        }
        self.flush();
    }

    pub fn clear(&self, group: &str, key: Option<&str>) -> usize {
        let n = {
            let mut g = self.inner.lock().unwrap();
            let before = g.pins.len();
            g.pins
                .retain(|(gr, k), _| !(gr == group && key.is_none_or(|x| x == k)));
            let n = before - g.pins.len();
            g.dirty |= n > 0;
            n
        };
        self.flush();
        n
    }

    pub fn event(&self, e: Event) {
        {
            let mut g = self.inner.lock().unwrap();
            g.events.push_front(e);
            g.events.truncate(MAX_EVENTS);
            g.dirty = true;
        }
        self.flush();
    }

    pub fn json(&self, group: Option<&str>, session: Option<&str>) -> Value {
        let t = now();
        let g = self.inner.lock().unwrap();
        let mut pins: Vec<&Pin> = g
            .pins
            .values()
            .filter(|p| p.expires_at().is_none_or(|e| e > t))
            .filter(|p| group.is_none_or(|x| x == p.group))
            .filter(|p| session.is_none_or(|x| x == p.session || p.session == GROUP_WIDE))
            .collect();
        pins.sort_by_key(|p| std::cmp::Reverse(p.last_used));
        let events: Vec<Value> = g
            .events
            .iter()
            .filter(|e| group.is_none_or(|x| x == e.group))
            .filter(|e| session.is_none_or(|x| x == e.session))
            .map(Event::json)
            .collect();
        json!({"pins": pins.iter().map(|p| p.json()).collect::<Vec<_>>(), "events": events})
    }

    fn to_file(g: &Inner) -> Value {
        let pins: Vec<Value> = g
            .pins
            .values()
            .map(|p| {
                let mut v = p.json();
                v["sig"] = json!(p.sig);
                v["ttl_minutes"] = json!(p.ttl_minutes);
                v
            })
            .collect();
        json!({"pins": pins, "events": g.events.iter().map(Event::json).collect::<Vec<_>>()})
    }

    pub fn flush(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let text = {
            let mut g = self.inner.lock().unwrap();
            if !g.dirty {
                return;
            }
            g.dirty = false;
            Self::to_file(&g).to_string()
        };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text)
            .and_then(|_| std::fs::rename(&tmp, path))
            .is_err()
        {
            crate::state::log(
                "pins_save_failed",
                json!({"path": path.display().to_string()}),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Vec<String> {
        vec!["a/1".into(), "b/2".into()]
    }

    #[test]
    fn set_get_expire_and_persist() {
        let dir = std::env::temp_dir().join(format!("modelgate-pins-{}", fastrand::u64(..)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pins.json");
        let p = Pins::load(Some(path.clone()));
        p.set(
            "g",
            "s1",
            "b/2",
            Scope::Conversation,
            10,
            "answered",
            &list(),
        );
        assert_eq!(
            p.get("g", "s1", &list(), Scope::Conversation)
                .unwrap()
                .model,
            "b/2"
        );
        assert!(p.get("g", "s2", &list(), Scope::Conversation).is_none());
        assert!(
            p.get("g", "s1", &["a/1".to_owned()], Scope::Conversation)
                .is_none(),
            "a pin whose model left the group is dropped"
        );
        p.set(
            "g",
            "s1",
            "b/2",
            Scope::Conversation,
            10,
            "answered",
            &list(),
        );
        p.event(Event {
            time: 1,
            group: "g".into(),
            session: "s1".into(),
            from: "a/1".into(),
            to: Some("b/2".into()),
            reason: "timeout".into(),
        });
        let again = Pins::load(Some(path.clone()));
        assert_eq!(
            again
                .get("g", "s1", &list(), Scope::Conversation)
                .unwrap()
                .model,
            "b/2"
        );
        let j = again.json(Some("g"), None);
        assert_eq!(j["events"][0]["reason"], "timeout");
        assert_eq!(again.clear("g", None), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn until_changed_drops_on_list_change_and_day_expiry() {
        let p = Pins::load(None);
        p.set(
            "g",
            GROUP_WIDE,
            "a/1",
            Scope::UntilChanged,
            0,
            "answered",
            &list(),
        );
        assert!(p
            .get("g", GROUP_WIDE, &list(), Scope::UntilChanged)
            .is_some());
        let changed = vec!["a/1".to_owned(), "c/3".to_owned()];
        assert!(p
            .get("g", GROUP_WIDE, &changed, Scope::UntilChanged)
            .is_none());
        let t = 1_790_000_000;
        let m = next_ist_midnight(t);
        assert!(m > t && m - t <= 86_400);
        assert_eq!((m + IST) % 86_400, 0);
        let mut pin = Pin {
            group: "g".into(),
            session: "*".into(),
            model: "a/1".into(),
            scope: "conversation".into(),
            source: String::new(),
            sig: String::new(),
            set_at: 100,
            last_used: 100,
            ttl_minutes: 2,
        };
        assert_eq!(pin.expires_at(), Some(220));
        pin.scope = "until_changed".into();
        assert_eq!(pin.expires_at(), None);
    }
}
