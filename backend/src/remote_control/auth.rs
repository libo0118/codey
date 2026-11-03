use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    num::NonZeroU32,
    path::PathBuf,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const ITERATIONS: u32 = 600_000;
const MAX_SESSIONS: usize = 32;
pub(super) const REMEMBER_SECONDS: i64 = 30 * 24 * 60 * 60;
const SESSION_SECONDS: i64 = 12 * 60 * 60;
const MAX_STORE: u64 = 256 * 1024;
const BAD_LOGIN: &str = "账号或密码错误";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Credentials {
    username: String,
    salt: String,
    hash: String,
    iterations: u32,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Session {
    id: String,
    digest: String,
    created_at: i64,
    expires_at: i64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    version: u32,
    revision: String,
    enabled: bool,
    port: u16,
    public_url: String,
    credentials: Option<Credentials>,
    sessions: Vec<Session>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            version: 1,
            revision: String::new(),
            enabled: false,
            port: 18787,
            public_url: String::new(),
            credentials: None,
            sessions: Vec::new(),
        }
    }
}

pub(super) struct Auth {
    path: PathBuf,
    document: Document,
    fingerprint: Option<String>,
    attempts: HashMap<IpAddr, VecDeque<Instant>>,
    global_attempts: VecDeque<Instant>,
}

pub(super) struct Login {
    pub token: String,
    pub session: Value,
}

fn secret(bytes: usize) -> Result<String, String> {
    let mut value = vec![0; bytes];
    SystemRandom::new()
        .fill(&mut value)
        .map_err(|_| "安全随机数生成失败")?;
    Ok(URL_SAFE_NO_PAD.encode(value))
}

fn digest(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn iso(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .expect("validated remote timestamp")
        .to_rfc3339()
}

pub(super) fn csrf(token: &str) -> String {
    digest(&format!("codey-remote-csrf:{token}"))
}

pub(super) fn csrf_matches(token: &str, supplied: &str) -> bool {
    let expected = csrf(token);
    expected.len() == supplied.len()
        && expected
            .as_bytes()
            .iter()
            .zip(supplied.as_bytes())
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
}

impl Credentials {
    pub fn new(username: &str, password: &str) -> Result<Self, String> {
        let username = username.trim();
        if username.is_empty()
            || username.chars().count() > 128
            || username.chars().any(char::is_control)
        {
            return Err("账号须为1–128个字符且不能包含控制字符".into());
        }
        if password.chars().count() < 12 || password.len() > 1024 {
            return Err("密码至少12个字符，且不能超过1024字节".into());
        }
        let salt = secret(16)?;
        let bytes = URL_SAFE_NO_PAD
            .decode(&salt)
            .map_err(|_| "密码盐生成失败")?;
        let mut hash = [0u8; 32];
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            NonZeroU32::new(ITERATIONS).unwrap(),
            &bytes,
            password.as_bytes(),
            &mut hash,
        );
        Ok(Self {
            username: username.into(),
            salt,
            hash: URL_SAFE_NO_PAD.encode(hash),
            iterations: ITERATIONS,
        })
    }

    pub fn verify(&self, username: &str, password: &str) -> bool {
        if password.len() > 1024 {
            return false;
        }
        let (Ok(salt), Ok(hash)) = (
            URL_SAFE_NO_PAD.decode(&self.salt),
            URL_SAFE_NO_PAD.decode(&self.hash),
        ) else {
            return false;
        };
        let verified = pbkdf2::verify(
            pbkdf2::PBKDF2_HMAC_SHA256,
            NonZeroU32::new(self.iterations).unwrap(),
            &salt,
            password.as_bytes(),
            &hash,
        )
        .is_ok();
        // Always perform the KDF, including unknown usernames.
        verified && username.trim() == self.username
    }
}

impl Auth {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let bytes = match std::fs::symlink_metadata(&path) {
            Ok(_) => Some(
                crate::fs_util::read_bounded(&path, MAX_STORE)
                    .map_err(|_| "远程账号文件不可读取，服务未启动")?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("远程账号文件不可读取，服务未启动".into()),
        };
        let document: Document = match bytes.as_ref() {
            Some(bytes) => {
                serde_json::from_slice(bytes).map_err(|_| "远程账号文件损坏，服务未启动")?
            }
            None => Document::default(),
        };
        let fingerprint = bytes
            .as_ref()
            .map(|bytes| crate::fs_util::sha256_hex(bytes));
        let valid_credentials = document.credentials.as_ref().is_none_or(|credential| {
            !credential.username.trim().is_empty()
                && credential.username.chars().count() <= 128
                && !credential.username.chars().any(char::is_control)
                && credential.iterations == ITERATIONS
                && URL_SAFE_NO_PAD
                    .decode(&credential.salt)
                    .is_ok_and(|value| value.len() == 16)
                && URL_SAFE_NO_PAD
                    .decode(&credential.hash)
                    .is_ok_and(|value| value.len() == 32)
        });
        if document.version != 1
            || document.port == 0
            || !valid_credentials
            || (document.credentials.is_some()
                && uuid::Uuid::parse_str(&document.revision).is_err())
            || document.sessions.len() > MAX_SESSIONS
            || (document.credentials.is_none()
                && (document.enabled || !document.sessions.is_empty()))
            || document.sessions.iter().any(|session| {
                session.created_at < 0
                    || session.expires_at <= session.created_at
                    || session.expires_at - session.created_at > REMEMBER_SECONDS
                    || chrono::DateTime::from_timestamp(session.expires_at, 0).is_none()
                    || !URL_SAFE_NO_PAD
                        .decode(&session.digest)
                        .is_ok_and(|bytes| bytes.len() == 32)
                    || uuid::Uuid::parse_str(&session.id).is_err()
            })
        {
            return Err("远程账号文件格式无效，服务未启动".into());
        }
        super::server::validate_public_url(&document.public_url)?;
        Ok(Self {
            path,
            document,
            fingerprint,
            attempts: HashMap::new(),
            global_attempts: VecDeque::new(),
        })
    }

    fn current_fingerprint(&self) -> Result<Option<String>, String> {
        match std::fs::symlink_metadata(&self.path) {
            Ok(_) => crate::fs_util::read_bounded(&self.path, MAX_STORE)
                .map(|bytes| Some(crate::fs_util::sha256_hex(&bytes)))
                .map_err(|_| "远程账号文件不可读取，请在本机检查".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("远程账号文件不可读取，请在本机检查".into()),
        }
    }

    pub fn ensure_current(&self) -> Result<(), String> {
        if self.current_fingerprint()? != self.fingerprint {
            return Err("远程账号文件已被外部修改，请重启Codey后重新加载".into());
        }
        Ok(())
    }

    fn commit(
        &mut self,
        update: impl FnOnce(&mut Document) -> Result<bool, String>,
    ) -> Result<(), String> {
        use fs2::FileExt;
        let parent = self.path.parent().ok_or("远程账号保存路径无效")?;
        std::fs::create_dir_all(parent).map_err(|_| "创建远程账号目录失败")?;
        let lock_path = self.path.with_extension("lock");
        if let Ok(metadata) = std::fs::symlink_metadata(&lock_path)
            && (!metadata.is_file() || metadata.file_type().is_symlink())
        {
            return Err("远程账号锁文件无效".into());
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(&lock_path).map_err(|_| "打开远程账号锁失败")?;
        lock.try_lock_exclusive()
            .map_err(|_| "其他远程账号操作正在执行，请重试")?;
        if !crate::fs_util::locked_file_matches_path(&lock, &lock_path) {
            return Err("远程账号锁已变化，请重试".into());
        }
        let mut current = Self::load(self.path.clone())?;
        // Session-only changes may merge; credential or listener changes require a fresh revision.
        if current.document.revision != self.document.revision
            || current.document.credentials != self.document.credentials
            || current.document.enabled != self.document.enabled
            || current.document.port != self.document.port
            || current.document.public_url != self.document.public_url
        {
            return Err("远程设置已更新，请刷新后重试".into());
        }
        if update(&mut current.document)? {
            let bytes =
                serde_json::to_vec_pretty(&current.document).map_err(|_| "远程账号保存失败")?;
            crate::fs_util::atomic_write_private_with_parent(&self.path, &bytes)
                .map_err(|_| "远程账号保存失败，原设置未改变")?;
            current.fingerprint = Some(crate::fs_util::sha256_hex(&bytes));
        }
        self.fingerprint = current.fingerprint;
        self.document = current.document;
        Ok(())
    }

    pub fn port(&self) -> u16 {
        self.document.port
    }
    pub fn public_url(&self) -> &str {
        &self.document.public_url
    }
    pub fn enabled(&self) -> bool {
        self.document.enabled
    }
    pub fn configured(&self) -> bool {
        self.document.credentials.is_some()
    }
    pub fn revision(&self) -> &str {
        &self.document.revision
    }
    pub fn credentials(&self) -> Option<Credentials> {
        self.document.credentials.clone()
    }

    pub fn configure(
        &mut self,
        revision: &str,
        username: &str,
        credentials: Option<Credentials>,
        port: u16,
        public_url: String,
    ) -> Result<(), String> {
        if port == 0 {
            return Err("端口须为1–65535".into());
        }
        let username = username.trim();
        self.commit(|next| {
            if revision != next.revision {
                return Err("远程设置已更新，请刷新后重试".into());
            }
            let changed_identity = credentials.is_some()
                || next
                    .credentials
                    .as_ref()
                    .is_some_and(|old| old.username != username);
            if let Some(credentials) = credentials {
                next.credentials = Some(credentials);
            }
            let account = next
                .credentials
                .as_mut()
                .ok_or("请先设置管理员账号和至少12个字符的密码")?;
            if username.is_empty()
                || username.chars().count() > 128
                || username.chars().any(char::is_control)
            {
                return Err("账号格式无效".into());
            }
            account.username = username.to_string();
            if !changed_identity && next.port == port && next.public_url == public_url {
                return Ok(false);
            }
            if changed_identity {
                next.sessions.clear();
            }
            next.port = port;
            next.public_url = public_url;
            next.revision = uuid::Uuid::new_v4().to_string();
            Ok(true)
        })
    }

    pub fn set_enabled(&mut self, enabled: bool) -> Result<(), String> {
        self.commit(|next| {
            if enabled && next.credentials.is_none() {
                return Err("请先设置管理员账号和密码".into());
            }
            if next.enabled == enabled {
                return Ok(false);
            }
            next.enabled = enabled;
            next.revision = uuid::Uuid::new_v4().to_string();
            Ok(true)
        })
    }

    pub fn attempt(&mut self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let recent = |time: &Instant| now.duration_since(*time) < Duration::from_secs(60);
        self.global_attempts.retain(recent);
        self.attempts.retain(|_, times| {
            times.retain(recent);
            !times.is_empty()
        });
        if self.global_attempts.len() >= 30
            || (!self.attempts.contains_key(&ip) && self.attempts.len() >= 128)
        {
            return false;
        }
        let attempts = self.attempts.entry(ip).or_default();
        if attempts.len() >= 10 {
            return false;
        }
        attempts.push_back(now);
        self.global_attempts.push_back(now);
        true
    }

    pub fn issue(&mut self, expected_revision: &str, remember: bool) -> Result<Login, String> {
        if self.document.revision != expected_revision || !self.configured() {
            return Err(BAD_LOGIN.into());
        }
        let token = secret(32)?;
        self.commit(|next| {
            let now = now();
            next.sessions.retain(|session| session.expires_at > now);
            if next.sessions.len() >= MAX_SESSIONS {
                return Err("登录会话已达上限，请在本机撤销旧会话".into());
            }
            next.sessions.push(Session {
                id: uuid::Uuid::new_v4().to_string(),
                digest: digest(&token),
                created_at: now,
                expires_at: now
                    + if remember {
                        REMEMBER_SECONDS
                    } else {
                        SESSION_SECONDS
                    },
            });
            Ok(true)
        })?;
        Ok(Login {
            session: self.session(&token),
            token,
        })
    }

    pub fn authorized(&self, token: &str) -> bool {
        self.ensure_current().is_ok()
            && URL_SAFE_NO_PAD
                .decode(token)
                .is_ok_and(|bytes| bytes.len() == 32)
            && self
                .document
                .sessions
                .iter()
                .any(|session| session.digest == digest(token) && session.expires_at > now())
    }

    pub fn session(&self, token: &str) -> Value {
        if self.authorized(token) {
            let session = self
                .document
                .sessions
                .iter()
                .find(|entry| entry.digest == digest(token))
                .unwrap();
            json!({"authenticated":true,"username":self.document.credentials.as_ref().unwrap().username,
                "csrfToken":csrf(token),"expiresAt":iso(session.expires_at)})
        } else {
            json!({"authenticated":false})
        }
    }

    pub fn logout(&mut self, token: &str) -> Result<(), String> {
        self.commit(|next| {
            next.sessions
                .retain(|session| session.digest != digest(token));
            Ok(true)
        })
    }

    pub fn revoke(&mut self, id: &str) -> Result<(), String> {
        self.commit(|next| {
            next.sessions
                .retain(|session| id != "*" && session.id != id);
            Ok(true)
        })
    }

    pub fn status(&self) -> Value {
        json!({"port":self.document.port,"publicUrl":self.document.public_url,"accountConfigured":self.configured(),
            "username":self.document.credentials.as_ref().map(|account|account.username.as_str()),"revision":self.document.revision,
            "sessions":self.document.sessions.iter().filter(|session|session.expires_at>now()).map(|session|json!({"id":session.id,
                "createdAt":iso(session.created_at),"expiresAt":iso(session.expires_at)})).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(path: PathBuf) -> Auth {
        let mut auth = Auth::load(path).unwrap();
        auth.configure(
            "",
            "admin",
            Some(Credentials::new("admin", "test-password-1234").unwrap()),
            18787,
            String::new(),
        )
        .unwrap();
        auth
    }

    #[test]
    fn password_verification_uses_salted_kdf_and_generic_rejection() {
        assert!(Credentials::new("admin", "short").is_err());
        let account = Credentials::new("admin", "test-password-1234").unwrap();
        assert!(account.verify("admin", "test-password-1234"));
        assert!(!account.verify("unknown", "test-password-1234"));
        assert!(!account.verify("admin", "wrong"));
        assert_ne!(
            account.salt,
            Credentials::new("admin", "test-password-1234")
                .unwrap()
                .salt
        );
    }

    #[test]
    fn remembered_sessions_survive_restart_and_are_not_serialized_as_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote.json");
        let mut auth = configured(path.clone());
        let revision = auth.revision().to_owned();
        let login = auth.issue(&revision, true).unwrap();
        let public = auth.status().to_string();
        let stored = std::fs::read_to_string(&path).unwrap();
        assert!(!stored.contains(&login.token));
        assert!(!stored.contains("test-password-1234"));
        assert!(!public.contains("salt"));
        assert!(!public.contains("hash"));
        let mut reloaded = Auth::load(path.clone()).unwrap();
        assert!(reloaded.authorized(&login.token));
        assert!(csrf_matches(
            &login.token,
            login.session["csrfToken"].as_str().unwrap()
        ));
        assert!(!csrf_matches(&login.token, "wrong"));
        reloaded.logout(&login.token).unwrap();
        assert!(!Auth::load(path).unwrap().authorized(&login.token));
    }

    #[test]
    fn password_change_stale_revision_and_revocation_invalidate_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote.json");
        let mut auth = configured(path.clone());
        let revision = auth.revision().to_owned();
        let login = auth.issue(&revision, false).unwrap();
        assert_eq!(auth.revision(), revision);
        assert!(
            auth.configure("stale", "admin", None, 12345, String::new())
                .is_err()
        );
        auth.configure(
            &revision,
            "admin",
            Some(Credentials::new("admin", "updated-password-1234").unwrap()),
            18787,
            String::new(),
        )
        .unwrap();
        assert!(!auth.authorized(&login.token));
        assert!(auth.issue(&revision, true).is_err());
        let login = auth.issue(&auth.revision().to_owned(), true).unwrap();
        auth.revoke("*").unwrap();
        assert!(!Auth::load(path).unwrap().authorized(&login.token));
    }

    #[test]
    fn defaults_and_broken_stores_fail_closed_and_rate_limits_are_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote.json");
        let mut auth = Auth::load(path.clone()).unwrap();
        assert_eq!(auth.port(), 18787);
        assert!(!auth.enabled());
        assert!(!auth.configured());
        assert!(auth.set_enabled(true).is_err());
        for _ in 0..10 {
            assert!(auth.attempt("127.0.0.1".parse().unwrap()));
        }
        assert!(!auth.attempt("127.0.0.1".parse().unwrap()));
        std::fs::write(path.clone(), b"broken").unwrap();
        assert!(Auth::load(path).is_err());
    }

    #[test]
    fn failed_persistence_does_not_publish_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote.json");
        let mut auth = Auth::load(path.clone()).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(
            auth.configure(
                "",
                "admin",
                Some(Credentials::new("admin", "test-password-1234").unwrap()),
                18787,
                String::new()
            )
            .is_err()
        );
        assert!(!auth.configured());
    }

    #[test]
    fn independent_session_writers_preserve_settings_revision_and_merge_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote.json");
        let mut first = configured(path.clone());
        let mut second = Auth::load(path.clone()).unwrap();
        let mut third = Auth::load(path.clone()).unwrap();
        let revision = first.revision().to_owned();
        let first_login = first.issue(&revision, true).unwrap();
        assert!(!second.authorized(&first_login.token));
        second
            .configure(&revision, "admin", None, 18787, String::new())
            .unwrap();
        assert_eq!(second.revision(), revision);
        assert!(second.authorized(&first_login.token));
        let second_login = third.issue(&revision, false).unwrap();
        let reloaded = Auth::load(path.clone()).unwrap();
        assert_eq!(reloaded.revision(), revision);
        assert!(reloaded.authorized(&first_login.token));
        assert!(reloaded.authorized(&second_login.token));

        second
            .configure(&revision, "admin", None, 18788, String::new())
            .unwrap();
        let reloaded = Auth::load(path).unwrap();
        assert_ne!(reloaded.revision(), revision);
        assert_eq!(reloaded.port(), 18788);
        assert!(reloaded.authorized(&first_login.token));
        assert!(reloaded.authorized(&second_login.token));
    }

    #[test]
    fn independent_setting_writers_cannot_restore_old_credentials_or_reuse_revisions() {
        for (username, password, port, public_url) in [
            ("admin", Some("updated-password-1234"), 18787, ""),
            ("renamed", None, 18787, ""),
            ("admin", None, 18788, ""),
            ("admin", None, 18787, "https://example.com"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("remote.json");
            let mut first = configured(path.clone());
            let revision = first.revision().to_owned();
            let login = first.issue(&revision, true).unwrap();
            let mut stale = Auth::load(path.clone()).unwrap();
            let old_credentials = stale.credentials();
            first
                .configure(
                    &revision,
                    username,
                    password.map(|password| Credentials::new(username, password).unwrap()),
                    port,
                    public_url.into(),
                )
                .unwrap();
            assert_ne!(first.revision(), revision);
            let stored = std::fs::read(&path).unwrap();
            assert!(
                stale
                    .configure(&revision, "admin", None, 18787, String::new())
                    .is_err()
            );
            assert!(
                stale
                    .configure(&revision, "admin", old_credentials, 18787, String::new())
                    .is_err()
            );
            assert!(stale.issue(&revision, true).is_err());
            assert!(stale.set_enabled(true).is_err());
            assert!(stale.logout(&login.token).is_err());
            assert!(stale.revoke("*").is_err());
            assert_eq!(std::fs::read(&path).unwrap(), stored);
            let mut reloaded = Auth::load(path.clone()).unwrap();
            let current_revision = reloaded.revision().to_owned();
            reloaded
                .configure(&current_revision, username, None, port, public_url.into())
                .unwrap();
            assert_eq!(reloaded.revision(), current_revision);
            assert!(
                reloaded
                    .credentials()
                    .unwrap()
                    .verify(username, password.unwrap_or("test-password-1234"))
            );
            if password.is_some() {
                assert!(
                    !reloaded
                        .credentials()
                        .unwrap()
                        .verify(username, "test-password-1234")
                );
            }
            assert_eq!(
                reloaded.authorized(&login.token),
                password.is_none() && username == "admin"
            );
        }
    }

    #[test]
    fn independent_session_writers_cannot_restore_logged_out_or_revoked_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote.json");
        let mut first = configured(path.clone());
        let revision = first.revision().to_owned();
        let first_login = first.issue(&revision, true).unwrap();
        let mut second = Auth::load(path.clone()).unwrap();
        first.logout(&first_login.token).unwrap();
        let second_login = second.issue(&revision, true).unwrap();
        let third_login = second.issue(&revision, false).unwrap();
        first.revoke(&second.document.sessions[0].id).unwrap();
        let reloaded = Auth::load(path.clone()).unwrap();
        assert_eq!(reloaded.revision(), revision);
        assert!(!reloaded.authorized(&first_login.token));
        assert!(!reloaded.authorized(&second_login.token));
        assert!(reloaded.authorized(&third_login.token));
        second.revoke("*").unwrap();
        first.set_enabled(false).unwrap();
        assert_eq!(first.revision(), revision);
        assert!(first.document.sessions.is_empty());
        let new_login = second.issue(&revision, true).unwrap();
        first.set_enabled(true).unwrap();
        let reloaded = Auth::load(path).unwrap();
        assert!(reloaded.enabled());
        assert_ne!(reloaded.revision(), revision);
        assert!(reloaded.authorized(&new_login.token));
        assert!(!reloaded.authorized(&first_login.token));
        assert!(!reloaded.authorized(&second_login.token));
        assert!(!reloaded.authorized(&third_login.token));
    }

    #[test]
    fn external_corruption_blocks_authorization_and_no_op_writes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote.json");
        let mut first = configured(path.clone());
        let revision = first.revision().to_owned();
        let login = first.issue(&revision, true).unwrap();
        assert!(Auth::load(path.clone()).unwrap().authorized(&login.token));
        std::fs::write(&path, b"corrupted").unwrap();
        assert!(!first.authorized(&login.token));
        assert!(
            first
                .configure(&revision, "admin", None, 18787, String::new())
                .is_err()
        );
        assert!(first.set_enabled(false).is_err());
        assert!(first.logout(&login.token).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupted");
    }

    #[test]
    fn external_setting_edits_cannot_reuse_the_current_revision() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote.json");
        let mut auth = configured(path.clone());
        let revision = auth.revision().to_owned();
        let login = auth.issue(&revision, true).unwrap();
        let mut changed = auth.document.clone();
        changed.credentials.as_mut().unwrap().username = "external".into();
        let stored = serde_json::to_vec_pretty(&changed).unwrap();
        std::fs::write(&path, &stored).unwrap();
        assert!(!auth.authorized(&login.token));
        assert!(auth.issue(&revision, true).is_err());
        assert!(
            auth.configure(&revision, "admin", None, 18787, String::new())
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), stored);
    }

    #[test]
    fn no_op_configuration_changes_require_the_store_lock() {
        use fs2::FileExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("remote.json");
        let mut auth = configured(path.clone());
        let revision = auth.revision().to_owned();
        let stored = std::fs::read(&path).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))
            .unwrap();
        lock.try_lock_exclusive().unwrap();
        assert!(
            auth.configure(&revision, "admin", None, 18787, String::new())
                .is_err()
        );
        assert!(auth.set_enabled(false).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), stored);
    }

    #[test]
    fn expiry_is_enforced_without_extending_the_remembered_lifetime() {
        let temp = tempfile::tempdir().unwrap();
        let mut auth = configured(temp.path().join("remote.json"));
        let login = auth.issue(&auth.revision().to_owned(), true).unwrap();
        assert_eq!(
            auth.document.sessions[0].expires_at - auth.document.sessions[0].created_at,
            REMEMBER_SECONDS
        );
        auth.commit(|next| {
            next.sessions[0].created_at = now() - SESSION_SECONDS;
            next.sessions[0].expires_at = now() - 1;
            Ok(true)
        })
        .unwrap();
        assert!(!auth.authorized(&login.token));
    }
}
