//! Deliberately excludes auth and storage relocation from ordinary configuration APIs.
use crate::*;
use serde_json::{Value, json};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminOptions {
    pub enabled: bool,
    pub listen: String,
    pub browser_base_url: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Editable {
    pub host: Host,
    pub manager: Manager,
    pub admin: AdminOptions,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub revision: Revision,
    pub settings: Editable,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Apply {
    pub revision: Revision,
}
impl Editable {
    pub fn from_config(c: &Config) -> Self {
        Self {
            host: c.host.clone(),
            manager: c.manager.clone(),
            admin: AdminOptions {
                enabled: c.admin.enabled,
                listen: c.admin.listen.clone(),
                browser_base_url: c.admin.browser_base_url.clone(),
            },
        }
    }
    pub fn merge(self, mut config: Config) -> Result<Config, Error> {
        if self.manager.host_enabled != config.manager.host_enabled {
            return Err(Error::Invalid(
                "manager.host_enabled is read-only here; use manager lifecycle controls".into(),
            ));
        }
        config.host = self.host;
        config.manager = self.manager;
        config.admin.enabled = self.admin.enabled;
        config.admin.listen = self.admin.listen;
        config.admin.browser_base_url = self.admin.browser_base_url;
        config.validate()?;
        if let Some(base) = &config.admin.browser_base_url {
            // Match the server's local-only browser origin contract before persistence.
            let uri: http::Uri = base
                .parse()
                .map_err(|_| Error::Invalid("admin.browser_base_url is invalid".into()))?;
            let local = uri.host().is_some_and(|h| {
                h == "localhost"
                    || h.trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            });
            if uri.scheme_str() != Some("http")
                || !local
                || uri.path() != "/"
                || uri.query().is_some()
                || uri.authority().is_some_and(|a| a.as_str().contains('@'))
            {
                return Err(Error::Invalid(
                    "admin.browser_base_url must be a plain HTTP loopback origin".into(),
                ));
            }
        }
        Ok(config)
    }
}
pub fn save(path: &Path, update: Update) -> Result<Snapshot, Error> {
    let store = Store::new(path);
    let snapshot = store.load_or_create()?;
    if snapshot.revision != update.revision {
        return Err(Error::Conflict);
    }
    let config = update.settings.merge(snapshot.config)?;
    store.save(&config, &update.revision)
}
pub fn values(config: &Config) -> BTreeMap<String, Option<Value>> {
    let h = &config.host;
    let mut fields = BTreeMap::from([
        ("host.origin".into(), h.origin.as_ref().map(|v| json!(v))),
        ("host.policy".into(), h.policy.as_ref().map(|v| json!(v))),
        (
            "host.relay_url".into(),
            h.relay_url.as_ref().map(|v| json!(v)),
        ),
        ("host.websockets".into(), h.websockets.map(|v| json!(v))),
        (
            "host.websocket_origin".into(),
            h.websocket_origin.as_ref().map(|v| json!(v)),
        ),
        (
            "host.control.listen".into(),
            h.control.listen.as_ref().map(|v| json!(v)),
        ),
        (
            "host.control.allow_remote".into(),
            h.control.allow_remote.map(|v| json!(v)),
        ),
    ]);
    for (section, value) in [
        ("manager", serde_json::to_value(&config.manager).unwrap()),
        (
            "admin",
            serde_json::to_value(&Editable::from_config(config).admin).unwrap(),
        ),
    ] {
        for (key, value) in value.as_object().unwrap() {
            fields.insert(format!("{section}.{key}"), Some(value.clone()));
        }
    }
    fields
}
pub fn view(
    path: &Path,
    data: &Path,
    startup: Option<&Settings>,
    live: Option<(&str, bool, &str)>,
) -> Result<Value, Error> {
    let snapshot = Store::new(path).load_or_create()?;
    let mut fields = Settings::new();
    for (key, saved) in values(&snapshot.config) {
        let mut entry = startup
            .and_then(|s| s.get(&key))
            .cloned()
            .unwrap_or_else(|| {
                setting(
                    None,
                    Value::Null,
                    Source::Unavailable,
                    Change::Restart,
                    false,
                )
            });
        entry.saved = saved;
        if let Some((policy, enabled, origin)) = live {
            let effective = match key.as_str() {
                "host.policy" => Some(json!(policy)),
                "host.websockets" => Some(json!(enabled)),
                "host.websocket_origin" => Some(json!(origin)),
                _ => None,
            };
            if let Some(value) = effective {
                let old = if key == "host.websockets" {
                    json!(entry.effective == "on" || entry.effective == true)
                } else {
                    entry.effective.clone()
                };
                if value != old || entry.source == Source::Default {
                    entry.source = Source::State;
                }
                entry.effective = value;
                entry.change = Change::Live;
            }
        }
        fields.insert(key, entry);
    }
    let overridden: Vec<_> = fields
        .iter()
        .filter(|(_, f)| matches!(f.source, Source::Cli | Source::Environment))
        .filter(|(_, f)| f.saved.as_ref().is_some_and(|s| *s != f.effective))
        .map(|(k, _)| k.clone())
        .collect();
    let pending: Vec<_> = fields
        .iter()
        .filter(|(_, f)| {
            f.source != Source::Unavailable && f.saved.as_ref().is_some_and(|s| *s != f.effective)
        })
        .map(|(k, _)| k.clone())
        .collect();
    Ok(
        json!({"revision": snapshot.revision, "saved": Editable::from_config(&snapshot.config), "fields": fields,
        "effective_available": startup.is_some(), "pending": pending, "overridden": overridden,
        "storage": {"config": path, "host_data": data}}),
    )
}

/// Recompute annotations after admin/manager runtime fields have been merged.
pub fn annotate(view: &mut Value) {
    let mut pending = Vec::new();
    let mut overridden = Vec::new();
    if let Some(fields) = view["fields"].as_object() {
        for (key, field) in fields {
            if field["source"] == "unavailable" {
                continue;
            }
            let target = if !field["saved"].is_null() {
                Some(field["saved"].clone())
            } else {
                match key.as_str() {
                    "host.origin" => Some(json!("http://127.0.0.1:8096")),
                    "host.relay_url" => Some(json!("")),
                    "host.control.listen" => Some(json!("127.0.0.1:47950")),
                    "host.control.allow_remote" => Some(json!(false)),
                    _ => None,
                }
            };
            if target.is_some_and(|v| v != field["effective"]) {
                pending.push(key.clone());
                if field["source"] == "cli" || field["source"] == "environment" {
                    overridden.push(key.clone());
                }
            }
        }
    }
    view["pending"] = json!(pending);
    view["overridden"] = json!(overridden);
}

pub fn file_source(path: &Path, key: &str) -> Source {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Source::Unavailable;
    };
    let Ok(document) = text.parse::<toml::Value>() else {
        return Source::Unavailable;
    };
    if key
        .split('.')
        .try_fold(&document, |value, key| value.get(key))
        .is_some()
    {
        Source::File
    } else {
        Source::Default
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::test_support::TempDir;
    #[test]
    fn editing_preserves_credentials_rejects_stale_writers_and_lifecycle_changes() {
        let dir = TempDir::create();
        let path = dir.0.join("config.toml");
        let store = Store::new(&path);
        let snapshot = store.load_or_create().unwrap();
        let mut config = snapshot.config;
        config.admin.auth = Auth {
            username: Some("owner".into()),
            password_hash: Some("$argon2id$protected".into()),
        };
        let snapshot = store.save(&config, &snapshot.revision).unwrap();
        let mut editable = Editable::from_config(&snapshot.config);
        editable.host.origin = Some("http://localhost:8096".into());
        assert!(
            !serde_json::to_string(&editable)
                .unwrap()
                .contains("protected")
        );
        save(
            &path,
            Update {
                revision: snapshot.revision.clone(),
                settings: editable.clone(),
            },
        )
        .unwrap();
        assert!(matches!(
            save(
                &path,
                Update {
                    revision: snapshot.revision,
                    settings: editable
                }
            ),
            Err(Error::Conflict)
        ));
        let snapshot = store.load_or_create().unwrap();
        assert_eq!(
            snapshot.config.admin.auth.password_hash.as_deref(),
            Some("$argon2id$protected")
        );
        let mut editable = Editable::from_config(&snapshot.config);
        editable.manager.host_enabled = true;
        assert!(
            save(
                &path,
                Update {
                    revision: snapshot.revision,
                    settings: editable
                }
            )
            .is_err()
        );
    }
    #[test]
    fn removing_saved_startup_values_still_reports_pending_launch_overrides() {
        let mut view = json!({"fields": {"host.origin":{"saved":null,"effective":"http://localhost:9","source":"cli"},
            "host.websockets":{"saved":null,"effective":false,"source":"state"}}});
        annotate(&mut view);
        assert_eq!(view["pending"], json!(["host.origin"]));
        assert_eq!(view["overridden"], json!(["host.origin"]));
    }
}
