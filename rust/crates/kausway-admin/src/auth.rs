//! Credentials and ephemeral authorization. All methods run on bounded blocking workers.
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use kausway_config::{Auth, Snapshot, Store};
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;

pub const SETUP_LIFETIME: Duration = Duration::from_secs(15 * 60);
const SESSION_LIFETIME: Duration = Duration::from_secs(60 * 60);

pub type Failure = (u16, &'static str);
pub type Result<T> = std::result::Result<T, Failure>;
fn storage(_: impl std::fmt::Display) -> Failure {
    (
        409,
        "Could not save or load configuration; check permissions, validity, and concurrent edits.",
    )
}
fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn valid_hash(encoded: &str) -> Result<()> {
    let parsed = PasswordHash::new(encoded)
        .map_err(|_| (503, "Invalid administrator credential configuration."))?;
    let params = argon2::Params::try_from(&parsed)
        .map_err(|_| (503, "Invalid administrator password parameters."))?;
    if parsed.algorithm.as_str() != "argon2id"
        || parsed.version != Some(19)
        || parsed.salt.is_none()
        || parsed.hash.is_none()
        || !(8192..=65536).contains(&params.m_cost())
        || !(1..=5).contains(&params.t_cost())
        || !(1..=4).contains(&params.p_cost())
    {
        return Err((
            503,
            "Unsupported administrator password hash or work parameters.",
        ));
    }
    Ok(())
}
fn password_matches(password: &str, encoded: &str) -> bool {
    PasswordHash::new(encoded).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}
fn hash_password(password: &str) -> Result<String> {
    let mut salt = [0u8; 16];
    rand::rng().fill_bytes(&mut salt);
    let salt =
        SaltString::encode_b64(&salt).map_err(|_| (500, "Could not generate password salt."))?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| (500, "Could not hash password."))
}
fn validate(username: &str, password: &str) -> Result<()> {
    if username.is_empty()
        || username.len() > 64
        || username.trim() != username
        || username.chars().any(char::is_control)
    {
        return Err((
            400,
            "Username must contain 1–64 bytes without surrounding whitespace or control characters.",
        ));
    }
    if !(12..=1024).contains(&password.len()) {
        return Err((400, "Password must contain 12–1024 bytes."));
    }
    Ok(())
}
fn save_snapshot(
    store: &Store,
    mut snapshot: Snapshot,
    username: &str,
    password: &str,
) -> Result<()> {
    validate(username, password)?;
    snapshot.config.admin.auth = Auth {
        username: Some(username.into()),
        password_hash: Some(hash_password(password)?),
    };
    store
        .save(&snapshot.config, &snapshot.revision)
        .map_err(storage)?;
    Ok(())
}
/// Explicit local recovery can replace a malformed hash; it never opens web setup.
pub fn provision_offline(
    config: PathBuf,
    username: &str,
    password: &str,
    reset: bool,
) -> Result<()> {
    let store = Store::new(config);
    let snapshot = store.load_or_create().map_err(storage)?;
    if snapshot.config.admin.auth.username.is_some() && !reset {
        return Err((
            409,
            "Administrator already configured; use credentials reset explicitly.",
        ));
    }
    save_snapshot(&store, snapshot, username, password)
}

struct Session {
    csrf: String,
    expires: Instant,
}
#[derive(Serialize)]
pub struct Status {
    pub configured: bool,
    pub authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csrf: Option<String>,
}
pub struct Login {
    pub cookie: String,
    pub csrf: String,
}
pub struct State {
    store: Store,
    credentials: Option<[u8; 32]>,
    setup: Option<([u8; 32], Instant)>,
    sessions: HashMap<[u8; 32], Session>,
    attempts: u32,
    window: Instant,
}
impl State {
    pub fn new(config: PathBuf) -> Result<Self> {
        let mut state = Self {
            store: Store::new(config),
            credentials: None,
            setup: None,
            sessions: HashMap::new(),
            attempts: 0,
            window: Instant::now(),
        };
        state.refresh()?;
        Ok(state)
    }
    fn refresh(&mut self) -> Result<Snapshot> {
        let snapshot = self.store.load_or_create().map_err(storage)?;
        let auth = &snapshot.config.admin.auth;
        let fingerprint = match (&auth.username, &auth.password_hash) {
            (Some(username), Some(hash)) => {
                if username.is_empty() || username.len() > 64 {
                    return Err((503, "Invalid administrator username."));
                }
                valid_hash(hash)?;
                Some(digest(&format!("{username}\0{hash}")))
            }
            (None, None) => None,
            _ => return Err((503, "Incomplete administrator credential configuration.")),
        };
        if fingerprint != self.credentials {
            self.sessions.clear();
            self.setup = None;
            self.credentials = fingerprint;
        }
        self.sessions.retain(|_, s| s.expires > Instant::now());
        Ok(snapshot)
    }
    fn limit(&mut self) -> Result<()> {
        if self.window.elapsed() >= Duration::from_secs(60) {
            self.attempts = 0;
            self.window = Instant::now();
        }
        if self.attempts >= 10 {
            return Err((
                429,
                "Too many authentication attempts. Try again in one minute.",
            ));
        }
        self.attempts += 1;
        Ok(())
    }
    pub fn setup_link(&mut self, base: &str) -> Result<String> {
        self.refresh()?;
        if self.credentials.is_some() {
            return Err((
                409,
                "Administrator already configured. Use login or an explicit local reset.",
            ));
        }
        let token = random_token();
        self.setup = Some((digest(&token), Instant::now() + SETUP_LIFETIME));
        Ok(format!("{base}/setup#token={token}"))
    }
    pub fn status(&mut self, cookie: Option<&str>) -> Result<Status> {
        let snapshot = self.refresh()?;
        let session = cookie.and_then(|c| self.sessions.get(&digest(c)));
        Ok(Status {
            configured: self.credentials.is_some(),
            authenticated: session.is_some(),
            username: session.map(|_| snapshot.config.admin.auth.username.unwrap()),
            csrf: session.map(|s| s.csrf.clone()),
        })
    }
    pub fn setup(&mut self, token: &str, username: &str, password: &str) -> Result<()> {
        let snapshot = self.refresh()?;
        self.limit()?;
        if self.credentials.is_some() {
            return Err((409, "Administrator already configured."));
        }
        let presented = digest(token);
        if !self.setup.as_ref().is_some_and(|(expected, expiry)| {
            expected.ct_eq(&presented).into() && *expiry > Instant::now()
        }) {
            return Err((
                403,
                "Invalid or expired setup token. Obtain a new local setup link.",
            ));
        }
        self.save(snapshot, username, password)
    }
    fn save(&mut self, snapshot: Snapshot, username: &str, password: &str) -> Result<()> {
        save_snapshot(&self.store, snapshot, username, password)?;
        self.refresh()?;
        self.setup = None;
        self.sessions.clear();
        Ok(())
    }
    /// Restricted local CLI/IPC only. Initial provisioning never replaces an account.
    pub fn provision(&mut self, username: &str, password: &str, reset: bool) -> Result<()> {
        let snapshot = if reset {
            self.store.load_or_create().map_err(storage)?
        } else {
            self.refresh()?
        };
        if snapshot.config.admin.auth.username.is_some() && !reset {
            return Err((
                409,
                "Administrator already configured; use credentials reset explicitly.",
            ));
        }
        self.save(snapshot, username, password)
    }
    pub fn login(&mut self, username: &str, password: &str) -> Result<Login> {
        let snapshot = self.refresh()?;
        self.limit()?;
        if password.len() > 1024 || username.len() > 64 {
            return Err((401, "Invalid username or password."));
        }
        let auth = snapshot.config.admin.auth;
        let (Some(saved_user), Some(hash)) = (auth.username, auth.password_hash) else {
            return Err((409, "Complete administrator setup first."));
        };
        // Verify even when the username differs; do not provide username discovery errors.
        let correct_password = password_matches(password, &hash);
        if !correct_password || !bool::from(saved_user.as_bytes().ct_eq(username.as_bytes())) {
            return Err((401, "Invalid username or password."));
        }
        if self.sessions.len() >= 64 {
            return Err((
                429,
                "Too many active sessions; sign out or wait for expiry.",
            ));
        }
        let cookie = random_token();
        let csrf = random_token();
        self.sessions.insert(
            digest(&cookie),
            Session {
                csrf: csrf.clone(),
                expires: Instant::now() + SESSION_LIFETIME,
            },
        );
        Ok(Login { cookie, csrf })
    }
    fn authorize(&mut self, cookie: &str, csrf: &str) -> Result<Snapshot> {
        let snapshot = self.refresh()?;
        if !self
            .sessions
            .get(&digest(cookie))
            .is_some_and(|s| bool::from(s.csrf.as_bytes().ct_eq(csrf.as_bytes())))
        {
            return Err((403, "Session expired or invalid CSRF token."));
        }
        Ok(snapshot)
    }
    pub fn check_session(&mut self, cookie: &str, csrf: Option<&str>) -> Result<()> {
        let status = self.status(Some(cookie))?;
        if !status.authenticated {
            return Err((401, "Sign in to administer the host."));
        }
        if let Some(csrf) = csrf {
            self.authorize(cookie, csrf)?;
        }
        Ok(())
    }

    pub fn logout(&mut self, cookie: &str, csrf: &str) -> Result<()> {
        self.authorize(cookie, csrf)?;
        self.sessions.remove(&digest(cookie));
        Ok(())
    }
    pub fn change_password(
        &mut self,
        cookie: &str,
        csrf: &str,
        current: &str,
        username: &str,
        password: &str,
    ) -> Result<()> {
        let snapshot = self.authorize(cookie, csrf)?;
        self.limit()?;
        if current.len() > 1024
            || !password_matches(
                current,
                snapshot.config.admin.auth.password_hash.as_deref().unwrap(),
            )
        {
            return Err((401, "Current password is incorrect."));
        }
        self.save(snapshot, username, password)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "kw-auth-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn state(&self) -> State {
            State::new(self.0.join("config.toml")).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    const PASSWORD: &str = "correct horse battery staple";
    #[test]
    fn setup_expiry_replacement_single_use_and_hash_only_persistence() {
        let f = Fixture::new();
        let mut state = f.state();
        let old = state.setup_link("http://localhost:1234").unwrap();
        let new = state.setup_link("http://localhost:1234").unwrap();
        assert!(
            state
                .setup(old.split("#token=").nth(1).unwrap(), "alice", PASSWORD)
                .is_err()
        );
        let token = new.split("#token=").nth(1).unwrap();
        state.setup.as_mut().unwrap().1 = Instant::now() - Duration::from_secs(1);
        assert!(state.setup(token, "alice", PASSWORD).is_err());
        let new = state.setup_link("http://localhost:1234").unwrap();
        let token = new.split("#token=").nth(1).unwrap();
        state.setup(token, "alice", PASSWORD).unwrap();
        assert!(state.setup(token, "mallory", PASSWORD).is_err());
        assert!(state.setup_link("http://localhost:1234").is_err());
        let saved = fs::read_to_string(f.0.join("config.toml")).unwrap();
        assert!(saved.contains("$argon2id$"));
        assert!(!saved.contains(PASSWORD));
        assert!(!saved.contains(token));
    }
    #[test]
    fn sessions_csrf_password_changes_expiry_and_external_resets() {
        let f = Fixture::new();
        let mut state = f.state();
        state.provision("alice", PASSWORD, false).unwrap();
        assert!(state.login("wrong-user", PASSWORD).is_err());
        let first = state.login("alice", PASSWORD).unwrap();
        let second = state.login("alice", PASSWORD).unwrap();
        assert!(state.logout(&first.cookie, "wrong-csrf").is_err());
        assert!(
            state
                .change_password(
                    &first.cookie,
                    &first.csrf,
                    "wrong password",
                    "alice",
                    "another secure password"
                )
                .is_err()
        );
        state
            .change_password(
                &first.cookie,
                &first.csrf,
                PASSWORD,
                "bob",
                "another secure password",
            )
            .unwrap();
        assert!(!state.status(Some(&first.cookie)).unwrap().authenticated);
        assert!(!state.status(Some(&second.cookie)).unwrap().authenticated);
        assert!(state.provision("bob", PASSWORD, false).is_err());
        let login = state.login("bob", "another secure password").unwrap();
        state
            .sessions
            .get_mut(&digest(&login.cookie))
            .unwrap()
            .expires = Instant::now() - Duration::from_secs(1);
        assert!(!state.status(Some(&login.cookie)).unwrap().authenticated);
        let login = state.login("bob", "another secure password").unwrap();
        let mut other = f.state();
        other.provision("carol", PASSWORD, true).unwrap();
        assert!(!state.status(Some(&login.cookie)).unwrap().authenticated);
    }
    #[test]
    fn failed_save_keeps_setup_retryable_and_concurrent_setup_cannot_overwrite() {
        let f = Fixture::new();
        let mut first = f.state();
        let mut second = f.state();
        let a = first.setup_link("http://localhost:1234").unwrap();
        let b = second.setup_link("http://localhost:1234").unwrap();
        let original = f.0.join("config.toml");
        let backup = f.0.join("backup.toml");
        fs::rename(&original, &backup).unwrap();
        fs::create_dir(&original).unwrap();
        assert!(
            first
                .setup(a.split("#token=").nth(1).unwrap(), "alice", PASSWORD)
                .is_err()
        );
        assert!(first.setup.is_some());
        fs::remove_dir(&original).unwrap();
        fs::rename(&backup, &original).unwrap();
        first
            .setup(a.split("#token=").nth(1).unwrap(), "alice", PASSWORD)
            .unwrap();
        assert!(
            second
                .setup(b.split("#token=").nth(1).unwrap(), "mallory", PASSWORD)
                .is_err()
        );
    }
    #[test]
    fn invalid_hash_and_excessive_work_parameters_fail_closed() {
        assert!(valid_hash("$argon2id$broken").is_err());
        let hash = hash_password(PASSWORD).unwrap();
        assert!(valid_hash(&hash.replace("m=19456", "m=999999999")).is_err());
        assert!(valid_hash(&hash.replace("argon2id", "argon2i")).is_err());
    }
    #[test]
    fn login_work_is_rate_limited() {
        let f = Fixture::new();
        let mut state = f.state();
        for _ in 0..10 {
            assert_eq!(state.login("nobody", PASSWORD).err().unwrap().0, 409);
        }
        assert_eq!(state.login("nobody", PASSWORD).err().unwrap().0, 429);
        state.window = Instant::now() - Duration::from_secs(61);
        assert_eq!(state.login("nobody", PASSWORD).err().unwrap().0, 409);
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn malformed_hash_requires_explicit_reset_and_preserves_other_configuration() {
        let root = std::env::temp_dir().join(format!("kw-admin-recovery-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        std::fs::write(&path, "schema_version = 1\n[host]\norigin = 'http://localhost:8096'\n[admin.auth]\nusername = 'alice'\npassword_hash = '$argon2id$broken'\n").unwrap();
        assert!(State::new(path.clone()).is_err());
        assert!(provision_offline(path.clone(), "alice", "replacement password", false).is_err());
        provision_offline(path.clone(), "alice", "replacement password", true).unwrap();
        let mut state = State::new(path.clone()).unwrap();
        assert!(state.login("alice", "replacement password").is_ok());
        assert_eq!(
            Store::new(path)
                .load_or_create()
                .unwrap()
                .config
                .host
                .origin
                .as_deref(),
            Some("http://localhost:8096")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
