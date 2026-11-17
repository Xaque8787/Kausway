//! Authenticated saved/effective configuration. Existing live control endpoints remain compatible.
use crate::control::{ControlContext, Reply};
use http::StatusCode;
use kausway_config::{
    Settings, Source, Store,
    administration::{self, Apply, Update},
};
use kausway_transport::{net::RelayChoice, policy::TransportPolicy};
use serde_json::json;
use std::path::PathBuf;

pub struct Configuration {
    pub path: PathBuf,
    pub data_dir: PathBuf,
    pub startup: Settings,
}
fn failure(error: impl std::fmt::Display) -> Reply {
    Reply::error(StatusCode::CONFLICT, "configuration", &error.to_string())
}
fn configured(context: &ControlContext) -> Result<&Configuration, Reply> {
    context
        .configuration
        .as_ref()
        .ok_or_else(|| failure("This host has no shared configuration context"))
}
pub fn read(context: &ControlContext) -> Result<Reply, Reply> {
    let c = configured(context)?;
    let state = context.state.snapshot();
    administration::view(
        &c.path,
        &c.data_dir,
        Some(&c.startup),
        Some((
            state.policy.as_str(),
            state.websocket.enabled,
            state.websocket.origin_header.as_str(),
        )),
    )
    .map(|mut value| {
        value["transport_restart_required"] =
            json!(context.relay_transport_only != (state.policy == TransportPolicy::RelayOnly));
        administration::annotate(&mut value);
        Reply::json(StatusCode::OK, value)
    })
    .map_err(failure)
}
pub fn save(context: &ControlContext, body: &[u8]) -> Result<Reply, Reply> {
    let c = configured(context)?;
    let update: Update =
        serde_json::from_slice(body).map_err(|_| failure("Invalid configuration update"))?;
    administration::save(&c.path, update).map_err(failure)?;
    let config = read(context)?.body.unwrap();
    Ok(Reply::json(
        StatusCode::OK,
        json!({"saved": true, "applied": false, "configuration": config}),
    ))
}
pub fn apply(context: &ControlContext, body: &[u8]) -> Result<Reply, Reply> {
    let c = configured(context)?;
    let input: Apply =
        serde_json::from_slice(body).map_err(|_| failure("Invalid apply request"))?;
    let snapshot = Store::new(&c.path).load_or_create().map_err(failure)?;
    if snapshot.revision != input.revision {
        return Err(failure("Configuration changed; reload before applying"));
    }
    let h = snapshot.config.host;
    let live = context.state.snapshot();
    let fields = [
        ("host.policy", h.policy.is_some()),
        ("host.websockets", h.websockets.is_some()),
        ("host.websocket_origin", h.websocket_origin.is_some()),
    ];
    for (key, present) in fields {
        let unchanged = match key {
            "host.policy" => h.policy.as_deref() == Some(live.policy.as_str()),
            "host.websockets" => h.websockets == Some(live.websocket.enabled),
            "host.websocket_origin" => {
                h.websocket_origin.as_deref() == Some(live.websocket.origin_header.as_str())
            }
            _ => false,
        };
        if present
            && !unchanged
            && c.startup
                .get(key)
                .is_some_and(|f| matches!(f.source, Source::Cli | Source::Environment))
        {
            return Err(failure(format!(
                "{key} is overridden at launch; remove the override before applying saved configuration"
            )));
        }
    }
    let policy: Option<TransportPolicy> = h
        .policy
        .as_deref()
        .map(str::parse)
        .transpose()
        .map_err(failure)?;
    if policy == Some(TransportPolicy::RelayOnly) && context.relay == RelayChoice::Disabled {
        return Err(failure("relay_only requires an enabled relay"));
    }
    let origin = h
        .websocket_origin
        .as_deref()
        .map(str::parse)
        .transpose()
        .map_err(failure)?;
    Store::new(&c.path)
        .with_revision(&input.revision, || {
            context
                .state
                .set_live_configuration(policy, h.websockets, origin)
                .map_err(|e| kausway_config::Error::Invalid(e.to_string()))
        })
        .map_err(failure)?;
    let restart =
        context.relay_transport_only != (context.state.policy() == TransportPolicy::RelayOnly);
    Ok(Reply::json(
        StatusCode::OK,
        json!({"saved": true, "applied": true, "applied_revision": input.revision, "restart_required": restart, "configuration": read(context)?.body.unwrap()}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{control::route, fsutil::test_support::TempDir, state::HostState};
    use kausway_config::{Change, setting};
    use std::sync::Arc;
    fn fixture() -> (TempDir, ControlContext) {
        let dir = TempDir::create();
        let path = dir.0.join("config.toml");
        Store::new(&path).load_or_create().unwrap();
        let state = Arc::new(HostState::open(&dir.0.join("state.json"), None).unwrap());
        let id = crate::identity::load_or_create(&dir.0.join("identity.key"))
            .unwrap()
            .public();
        let mut context = ControlContext::new(
            state,
            Arc::default(),
            Box::new(move || iroh::EndpointAddr::new(id)),
            "http://127.0.0.1:8096/".into(),
            RelayChoice::Disabled,
            false,
            "test-auth-token",
        );
        context.configuration = Some(Configuration {
            path,
            data_dir: dir.0.clone(),
            startup: Settings::from([
                (
                    "host.origin".into(),
                    setting(
                        None,
                        json!("http://127.0.0.1:8096"),
                        Source::Default,
                        Change::Restart,
                        false,
                    ),
                ),
                (
                    "host.policy".into(),
                    setting(None, json!("auto"), Source::Default, Change::Live, false),
                ),
                (
                    "host.websockets".into(),
                    setting(None, json!("on"), Source::Default, Change::Live, false),
                ),
                (
                    "host.websocket_origin".into(),
                    setting(None, json!("rewrite"), Source::Default, Change::Live, false),
                ),
            ]),
        });
        (dir, context)
    }
    fn call(context: &ControlContext, method: &str, path: &str, body: serde_json::Value) -> Reply {
        route(
            context,
            method,
            path,
            Some("Bearer test-auth-token"),
            body.to_string().as_bytes(),
        )
    }
    #[test]
    fn save_apply_conflicts_and_unauthenticated_access() {
        let (_dir, context) = fixture();
        assert_eq!(
            route(&context, "GET", "/v1/config", None, b"").status,
            StatusCode::UNAUTHORIZED
        );
        let current = read(&context).unwrap().body.unwrap();
        let mut edit = json!({"revision":current["revision"],"settings":current["saved"]});
        edit["settings"]["host"]["websockets"] = json!(false);
        let saved = call(&context, "PUT", "/v1/config", edit.clone());
        assert_eq!(saved.status, StatusCode::OK);
        assert!(context.state.websocket().enabled);
        assert_eq!(
            call(&context, "PUT", "/v1/config", edit).status,
            StatusCode::CONFLICT
        );
        assert_eq!(
            call(
                &context,
                "POST",
                "/v1/config/apply",
                json!({"revision":current["revision"]})
            )
            .status,
            StatusCode::CONFLICT
        );
        let current = read(&context).unwrap().body.unwrap();
        assert_eq!(
            call(
                &context,
                "POST",
                "/v1/config/apply",
                json!({"revision":current["revision"]})
            )
            .status,
            StatusCode::OK
        );
        assert!(!context.state.websocket().enabled);
        let mut edit = json!({"revision":current["revision"],"settings":current["saved"]});
        edit["settings"]["admin"]["auth"] = json!({"username":"injected"});
        assert_eq!(
            call(&context, "PUT", "/v1/config", edit).status,
            StatusCode::CONFLICT
        );
    }
    #[test]
    fn launch_overrides_and_relay_constraints_prevent_misleading_apply() {
        let (_dir, mut context) = fixture();
        context
            .configuration
            .as_mut()
            .unwrap()
            .startup
            .get_mut("host.policy")
            .unwrap()
            .source = Source::Cli;
        let current = read(&context).unwrap().body.unwrap();
        let mut edit = json!({"revision":current["revision"],"settings":current["saved"]});
        edit["settings"]["host"]["policy"] = json!("p2p_only");
        assert_eq!(
            call(&context, "PUT", "/v1/config", edit).status,
            StatusCode::OK
        );
        let current = read(&context).unwrap().body.unwrap();
        assert_eq!(current["overridden"], json!(["host.policy"]));
        assert_eq!(
            call(
                &context,
                "POST",
                "/v1/config/apply",
                json!({"revision":current["revision"]})
            )
            .status,
            StatusCode::CONFLICT
        );
        assert_eq!(context.state.policy(), TransportPolicy::Auto);
        context
            .configuration
            .as_mut()
            .unwrap()
            .startup
            .get_mut("host.policy")
            .unwrap()
            .source = Source::Default;
        let mut edit = json!({"revision":current["revision"],"settings":current["saved"]});
        edit["settings"]["host"]["policy"] = json!("relay_only");
        assert_eq!(
            call(&context, "PUT", "/v1/config", edit).status,
            StatusCode::OK
        );
        let current = read(&context).unwrap().body.unwrap();
        assert_eq!(
            call(
                &context,
                "POST",
                "/v1/config/apply",
                json!({"revision":current["revision"]})
            )
            .status,
            StatusCode::CONFLICT
        );
    }
}
