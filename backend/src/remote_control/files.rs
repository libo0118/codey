//! Session-scoped file references and bounded private snapshots, adapted from
//! codex-mobile-bridge workspace/files.py and workspace.py (MIT).
//! See third_party/codex-mobile-bridge.LICENSE.
mod fs;
mod references;

use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::desktop::NativeProvenance;

pub(super) const CHUNK: usize = fs::CHUNK;
const CACHE_KIB: usize = 400 * 1024;
const MAX_CACHE_ENTRIES: usize = 64;
const LIFETIME: Duration = Duration::from_secs(10 * 60);
const MAX_INLINE_IMAGE_ENCODED: usize = (4 * 1024 * 1024usize).div_ceil(3) * 4;

#[derive(Clone)]
struct Binding {
    lease: u64,
    token: String,
    cwd: Option<String>,
    provenance: NativeProvenance,
    references: Result<references::References, String>,
}

struct ScopeAuthority {
    revision: u64,
    references: Result<HashMap<String, references::Reference>, String>,
}

struct Bindings {
    leases: Vec<Binding>,
    provenance: NativeProvenance,
    cwd: Result<Option<String>, String>,
    scopes: HashMap<references::Scope, ScopeAuthority>,
    allowed: HashMap<String, Vec<references::Reference>>,
    blocked: Option<String>,
}

impl Bindings {
    fn observe(&mut self, binding: &Binding) -> Result<(), String> {
        if self.provenance.owner != binding.provenance.owner {
            let error =
                "会话所属的 Codex 窗口已变化，请关闭此会话的连接后重新打开以重新授权".to_string();
            self.blocked = Some(error.clone());
            return Err(error);
        }
        if self.blocked.is_some() {
            return Ok(());
        }
        let Some(revision) = binding.provenance.revision else {
            self.blocked = Some("桌面文件版本未知，请关闭此会话的连接后重新打开以重新授权".into());
            return Ok(());
        };
        if self
            .provenance
            .revision
            .is_some_and(|current| revision < current)
        {
            return Ok(());
        }
        let fresh = self
            .provenance
            .revision
            .is_some_and(|current| revision > current)
            || self.leases.is_empty();
        if self
            .provenance
            .revision
            .is_some_and(|current| revision > current)
        {
            self.provenance.revision = Some(revision);
            self.cwd = Ok(binding.cwd.clone());
        } else if self.cwd.as_ref() != Ok(&binding.cwd) {
            self.cwd = Err("同版本桌面会话的工作区不一致".into());
        }
        let refs = match &binding.references {
            Ok(refs) => refs,
            Err(error) => {
                self.blocked = Some(error.clone());
                return Ok(());
            }
        };
        // Covered Native items can revoke references; unloaded history cannot.
        for (scope, authority) in &mut self.scopes {
            if !refs.covers(scope) {
                continue;
            }
            let values = refs.scopes.get(scope).cloned().unwrap_or_default();
            if revision == authority.revision && authority.references.as_ref() != Ok(&values) {
                authority.references = Err("同版本桌面会话的文件引用不一致".into());
            } else if revision > authority.revision {
                authority.revision = revision;
                authority.references = Ok(values);
            }
        }
        // A same-revision snapshot can confirm known authority, not introduce
        // unknown scopes after their last live lease has gone away.
        if fresh {
            for (scope, values) in &refs.scopes {
                self.scopes
                    .entry(scope.clone())
                    .or_insert_with(|| ScopeAuthority {
                        revision,
                        references: Ok(values.clone()),
                    });
            }
        }
        Ok(())
    }

    fn prune_scopes(&mut self) {
        self.scopes.retain(|scope, authority| {
            Some(authority.revision) == self.provenance.revision
                && authority
                    .references
                    .as_ref()
                    .is_ok_and(|refs| !refs.is_empty())
                || self.leases.iter().any(|binding| {
                    binding
                        .references
                        .as_ref()
                        .is_ok_and(|refs| refs.scopes.contains_key(scope))
                })
        });
        self.allowed.clear();
        for authority in self.scopes.values() {
            if let Ok(refs) = &authority.references {
                for (reference, value) in refs {
                    let values = self.allowed.entry(reference.clone()).or_default();
                    if !values.contains(value) {
                        values.push(value.clone());
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Protection {
    home: Option<PathBuf>,
    roots: Vec<PathBuf>,
}

impl Protection {
    fn environment() -> Self {
        let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
        let mut roots = Vec::new();
        if let Some(home) = &home {
            roots.extend([".codex", ".ssh", ".gnupg"].map(|name| home.join(name)));
        }
        if let Some(codex) = std::env::var_os("CODEX_HOME") {
            roots.push(PathBuf::from(codex));
        }
        #[cfg(windows)]
        for key in [
            "SystemRoot",
            "WINDIR",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramData",
        ] {
            if let Some(path) = std::env::var_os(key) {
                roots.push(PathBuf::from(path));
            }
        }
        Self { home, roots }
    }

    fn canonical(path: &std::path::Path) -> Result<Option<PathBuf>, String> {
        let path = references::native_path(path.to_str().ok_or("受保护目录路径编码无效")?)?;
        if !path.is_absolute() {
            return Err("受保护目录没有安全的本机路径".into());
        }
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("无法核验受保护目录的安全边界".into()),
            Ok(_) => {}
        }
        let canonical = path
            .canonicalize()
            .map_err(|_| "无法核验受保护目录的真实目标")?;
        if !std::fs::metadata(&canonical)
            .map_err(|_| "无法核验受保护目录的安全边界")?
            .is_dir()
        {
            return Err("受保护配置路径不是目录，拒绝下载".into());
        }
        Ok(Some(canonical))
    }

    fn check(&self, root: &std::path::Path, path: &std::path::Path) -> Result<(), String> {
        if let Some(home) = self
            .home
            .as_ref()
            .map(|path| Self::canonical(path))
            .transpose()?
            .flatten()
            && references::same_path(root, &home)
        {
            return Err("不能将用户主目录作为下载根".into());
        }
        self.check_path(root)?;
        self.check_path(path)
    }

    fn check_path(&self, path: &std::path::Path) -> Result<(), String> {
        if references::forbidden_file(path) {
            return Err("不支持下载系统或配置目录中的文件".into());
        }
        for protected in &self.roots {
            if references::within(path, protected) {
                return Err("不支持下载系统或配置目录中的文件".into());
            }
            if let Some(protected) = Self::canonical(protected)?
                && references::within(path, &protected)
            {
                return Err("不支持下载系统或配置目录真实目标中的文件".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Selection {
    owner: String,
    thread: String,
    reference: String,
    value: references::Reference,
    cwd: Option<String>,
    protection: Protection,
}

pub(super) struct Descriptor {
    pub size: u64,
    pub name: String,
    pub mime: &'static str,
    source: DescriptorSource,
    selection: Selection,
    created: Instant,
}

enum DescriptorSource {
    File { path: PathBuf, stamp: fs::Stamp },
    InlineImage,
}

impl Descriptor {
    pub fn selection(&self) -> Selection {
        self.selection.clone()
    }
    pub fn attachment(&self, id: &str) -> Value {
        let url = format!("/api/sessions/{}/files/{id}", self.selection.thread);
        let mut file =
            json!({"id":id,"name":self.name,"downloadUrl":url,"size":self.size,"mime":self.mime});
        if self.mime.starts_with("image/") {
            file["url"] = json!(url);
        }
        file
    }

    pub fn validate_source(&self) -> Result<(), String> {
        let DescriptorSource::File { path, stamp } = &self.source else {
            return Ok(());
        };
        let selected = selected_path(&self.selection)?;
        if !references::same_path(&selected, path)
            || &fs::stamp(&fs::open(&selected)?.file)? != stamp
        {
            return Err("文件内容或位置已变化，请重新解析下载".into());
        }
        Ok(())
    }
}

pub(super) struct Snapshot {
    pub file: File,
    pub size: u64,
    pub name: String,
    pub mime: &'static str,
    pub digest: String,
    _budget: OwnedSemaphorePermit,
}

impl Snapshot {
    pub fn read(&self, offset: u64) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0; CHUNK.min(self.size.saturating_sub(offset) as usize)];
        let count = fs::read_at(&self.file, offset, &mut bytes).map_err(|_| "下载快照读取失败")?;
        if count == 0 && offset < self.size {
            return Err("下载快照提前结束".into());
        }
        bytes.truncate(count);
        Ok(bytes)
    }
}

pub(super) struct Registry {
    bindings: HashMap<(String, String), Bindings>,
    descriptors: HashMap<String, Arc<Descriptor>>,
    budget: Arc<Semaphore>,
    next_lease: u64,
    protection: Protection,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            bindings: HashMap::new(),
            descriptors: HashMap::new(),
            budget: Arc::new(Semaphore::new(CACHE_KIB)),
            next_lease: 0,
            protection: Protection::environment(),
        }
    }
}

fn owner(token: &str) -> String {
    crate::fs_util::sha256_hex_str(token)
}

impl Registry {
    #[cfg(test)]
    pub fn with_protection(home: Option<PathBuf>, roots: Vec<PathBuf>) -> Self {
        Self {
            protection: Protection { home, roots },
            ..Self::default()
        }
    }
    pub fn selection(
        &self,
        token: &str,
        thread: &str,
        reference: &str,
    ) -> Result<Selection, String> {
        super::desktop::validate_thread(thread)?;
        if reference.is_empty() || reference.len() > 4096 || reference.chars().any(char::is_control)
        {
            return Err("文件引用无效".into());
        }
        let owner = owner(token);
        let bindings = self
            .bindings
            .get(&(owner.clone(), thread.to_string()))
            .ok_or("请先打开并保持连接此会话，再下载文件")?;
        if let Some(error) = &bindings.blocked {
            return Err(error.clone());
        }
        let mut selected = None;
        for binding in &bindings.leases {
            let values = binding.references.as_ref().map_err(Clone::clone)?;
            if let Some(value) = values.values.get(reference) {
                selected = Some((binding, reference.to_string(), value.clone()));
                break;
            }
        }
        if selected.is_none() {
            // Markdown renderers may percent-encode a public destination. Compare
            // lexical candidates only; this never probes a client-supplied path.
            let requested = references::local_path(reference)?;
            for binding in &bindings.leases {
                let values = binding.references.as_ref().map_err(Clone::clone)?;
                if let Some((key, value)) = values.values.iter().find(|(key, value)| {
                    matches!(value, references::Reference::File { value, native: false, .. } if key.as_str() == value.as_str())
                        && references::local_path(key)
                            .is_ok_and(|path| references::same_path(&path, &requested))
                }) {
                    selected = Some((binding, key.clone(), value.clone()));
                    break;
                }
            }
        }
        let (binding, reference, value) = selected.ok_or("文件引用不属于此会话的公开消息")?;
        let selection = Selection {
            owner,
            thread: thread.to_string(),
            reference,
            value,
            cwd: binding.cwd.clone(),
            protection: self.protection.clone(),
        };
        if !self.current(&selection) {
            return Err("会话连接的工作区或文件引用不一致，请重新打开会话".into());
        }
        Ok(selection)
    }

    pub fn current(&self, selection: &Selection) -> bool {
        self.bindings
            .get(&(selection.owner.clone(), selection.thread.clone()))
            .is_some_and(|bindings| {
                // History gaps may omit a reference, but cannot undo observed
                // Native revocation or disagree about authority/workspace.
                bindings.blocked.is_none()
                    && bindings.cwd.as_ref() == Ok(&selection.cwd)
                    && bindings
                        .allowed
                        .get(&selection.reference)
                        .is_some_and(|values| values.contains(&selection.value))
                    && bindings.leases.iter().all(|binding| {
                        binding.cwd == selection.cwd
                            && binding.references.as_ref().is_ok_and(|refs| {
                                refs.values
                                    .get(&selection.reference)
                                    .is_none_or(|value| value == &selection.value)
                            })
                    })
                    && bindings.leases.iter().any(|binding| {
                        binding.references.as_ref().is_ok_and(|refs| {
                            refs.values.get(&selection.reference) == Some(&selection.value)
                        })
                    })
            })
    }

    pub fn cached(&self, token: &str, thread: &str, id: &str) -> Result<Arc<Descriptor>, String> {
        let snapshot = self
            .descriptors
            .get(id)
            .ok_or("下载引用不存在或已过期，请重新解析")?;
        if snapshot.selection.owner != owner(token)
            || snapshot.selection.thread != thread
            || !self.current(&snapshot.selection)
            || snapshot.created.elapsed() >= LIFETIME
        {
            return Err("下载引用不属于此登录会话或文件引用已变化".into());
        }
        Ok(Arc::clone(snapshot))
    }

    pub fn budget(&mut self, size: u64) -> Result<OwnedSemaphorePermit, String> {
        let count = size.div_ceil(1024).max(1) as u32;
        self.budget
            .clone()
            .try_acquire_many_owned(count)
            .map_err(|_| "下载快照预算已满，请等待其他下载结束".into())
    }

    pub fn insert(&mut self, snapshot: Descriptor) -> Result<Value, String> {
        if !self.current(&snapshot.selection) {
            return Err("会话文件引用已变化，请重新解析".into());
        }
        if self.descriptors.len() >= MAX_CACHE_ENTRIES {
            if let Some(id) = self
                .descriptors
                .iter()
                .min_by_key(|(_, s)| s.created)
                .map(|(id, _)| id.clone())
            {
                self.descriptors.remove(&id);
            } else {
                return Err("下载引用数量已达上限".into());
            }
        }
        let mut random = [0u8; 32];
        use ring::rand::SecureRandom;
        ring::rand::SystemRandom::new()
            .fill(&mut random)
            .map_err(|_| "无法创建下载引用")?;
        let id = crate::fs_util::sha256_hex(&random);
        let attachment = snapshot.attachment(&id);
        self.descriptors.insert(id, Arc::new(snapshot));
        Ok(json!({"file":attachment}))
    }

    pub fn prune(&mut self, mut authorized: impl FnMut(&str) -> bool) {
        self.bindings.retain(|_, bindings| {
            bindings.leases.retain(|binding| authorized(&binding.token));
            bindings.prune_scopes();
            !bindings.leases.is_empty()
        });
        let remove: Vec<_> = self
            .descriptors
            .iter()
            .filter(|(_, snapshot)| {
                snapshot.created.elapsed() >= LIFETIME || !self.current(&snapshot.selection)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in remove {
            self.descriptors.remove(&id);
        }
    }

    pub fn clear(&mut self) {
        self.bindings.clear();
        self.descriptors.clear();
    }
}

pub(super) struct BindingGuard {
    registry: Arc<Mutex<Registry>>,
    key: (String, String),
    lease: u64,
}

impl BindingGuard {
    pub fn new(
        registry: Arc<Mutex<Registry>>,
        token: &str,
        thread: &str,
        state: &Value,
        provenance: &NativeProvenance,
    ) -> Result<Self, String> {
        if state["id"].as_str() != Some(thread) {
            return Err("文件投影会话不匹配".into());
        }
        let key = (owner(token), thread.to_string());
        let mut current = registry.lock().unwrap_or_else(|e| e.into_inner());
        current.next_lease += 1;
        let lease = current.next_lease;
        let binding = Binding {
            lease,
            token: token.to_string(),
            cwd: state["cwd"].as_str().map(str::to_string),
            provenance: provenance.clone(),
            references: references::collect(state),
        };
        let bindings = current
            .bindings
            .entry(key.clone())
            .or_insert_with(|| Bindings {
                leases: Vec::new(),
                provenance: provenance.clone(),
                cwd: Ok(binding.cwd.clone()),
                scopes: HashMap::new(),
                allowed: HashMap::new(),
                blocked: None,
            });
        if let Err(error) = bindings.observe(&binding) {
            current.prune(|_| true);
            return Err(error);
        }
        bindings.leases.push(binding);
        current.prune(|_| true);
        drop(current);
        Ok(Self {
            registry,
            key,
            lease,
        })
    }

    pub fn update(&self, state: &Value, provenance: &NativeProvenance) -> Result<(), String> {
        if state["id"].as_str() != Some(self.key.1.as_str()) {
            return Err("文件投影会话不匹配".into());
        }
        let mut current = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(bindings) = current.bindings.get_mut(&self.key)
            && let Some(index) = bindings
                .leases
                .iter()
                .position(|binding| binding.lease == self.lease)
        {
            if provenance.owner == bindings.leases[index].provenance.owner
                && let (Some(revision), Some(previous)) = (
                    provenance.revision,
                    bindings.leases[index].provenance.revision,
                )
                && revision < previous
            {
                bindings.blocked =
                    Some("桌面文件版本已重置，请关闭此会话的连接后重新打开以重新授权".into());
            }
            let binding = Binding {
                lease: self.lease,
                token: bindings.leases[index].token.clone(),
                cwd: state["cwd"].as_str().map(str::to_string),
                provenance: provenance.clone(),
                references: references::collect(state),
            };
            let observed = bindings.observe(&binding);
            if observed.is_ok() {
                bindings.leases[index] = binding;
            }
            current.prune(|_| true);
            return observed;
        }
        current.prune(|_| true);
        Ok(())
    }
}

impl Drop for BindingGuard {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(bindings) = registry.bindings.get_mut(&self.key) {
            bindings
                .leases
                .retain(|binding| binding.lease != self.lease);
        }
        registry.prune(|_| true);
    }
}

fn selected_path(selection: &Selection) -> Result<PathBuf, String> {
    let references::Reference::File {
        value,
        native,
        markdown,
    } = &selection.value
    else {
        return Err("内嵌图片没有本地文件路径".into());
    };
    let reference = if *markdown {
        references::local_path(value)?
    } else {
        references::native_path(value)?
    };
    if *native && reference.is_absolute() {
        selection.protection.check_path(&reference)?;
        let source = fs::open(&reference)?;
        let canonical = source.canonical(&reference)?;
        selection.protection.check_path(&canonical)?;
        if fs::stamp(&source.file)? != fs::stamp(&fs::open(&canonical)?.file)? {
            return Err("文件真实位置已变化".into());
        }
        return Ok(canonical);
    }
    // Relative native attachments still need the bound workspace boundary.
    let cwd = references::native_path(
        selection
            .cwd
            .as_deref()
            .ok_or("此会话没有本机工作目录，不支持下载")?,
    )?;
    if !cwd.is_absolute() || references::forbidden_workspace(&cwd) {
        return Err("此会话没有安全的本机工作区下载边界".into());
    }
    let _directory = fs::directory(&cwd)?;
    let root = cwd.canonicalize().map_err(|_| "会话工作目录不可访问")?;
    if references::forbidden_workspace(&root) {
        return Err("不能将用户主目录、系统或配置目录作为下载根".into());
    }
    let path = if reference.is_absolute() {
        reference
    } else {
        root.join(reference)
    };
    if !references::within(&path, &root) && !references::within(&path, &cwd) {
        return Err("文件不在此会话的安全工作区内".into());
    }
    if references::forbidden_workspace(&path) {
        return Err("不支持下载系统或配置目录中的文件".into());
    }
    selection.protection.check(&root, &path)?;
    let source = fs::open(&path)?;
    let canonical = source.canonical(&path)?;
    if !references::within(&canonical, &root)
        || fs::stamp(&source.file)? != fs::stamp(&fs::open(&canonical)?.file)?
    {
        return Err("文件真实位置已变化或超出工作区".into());
    }
    selection.protection.check(&root, &canonical)?;
    Ok(canonical)
}

pub(super) fn describe(
    selection: Selection,
    mut check: impl FnMut() -> Result<(), String>,
) -> Result<Descriptor, String> {
    check()?;
    if let references::Reference::InlineImage { url, name } = &selection.value {
        let metadata = inline_image_metadata(url)?;
        check()?;
        return Ok(Descriptor {
            size: metadata.size,
            name: name.clone(),
            mime: metadata.mime,
            source: DescriptorSource::InlineImage,
            selection,
            created: Instant::now(),
        });
    }
    let path = selected_path(&selection)?;
    let mut source = fs::open(&path)?;
    let before = fs::stamp(&source.file)?;
    if before.size() > fs::MAX_FILE {
        return Err("文件超过 200 MiB 下载上限".into());
    }
    let mut header = [0u8; 12];
    let mut count = 0;
    while count < header.len() {
        let read = source
            .file
            .read(&mut header[count..])
            .map_err(|_| "读取文件格式失败")?;
        if read == 0 {
            break;
        }
        count += read;
    }
    let mime = fs::image_mime(&header[..count]).unwrap_or("application/octet-stream");
    check()?;
    if selected_path(&selection)? != path || fs::stamp(&fs::open(&path)?.file)? != before {
        return Err("文件内容或位置已变化，请重新解析下载".into());
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or("文件名无效")?;
    Ok(Descriptor {
        size: before.size(),
        name,
        mime,
        source: DescriptorSource::File {
            path,
            stamp: before,
        },
        selection,
        created: Instant::now(),
    })
}

pub(super) fn prepare_snapshot(
    registry: &Arc<Mutex<Registry>>,
    descriptor: &Descriptor,
    mut check: impl FnMut() -> Result<(), String>,
) -> Result<Snapshot, String> {
    check()?;
    descriptor.validate_source()?;
    let budget = registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .budget(descriptor.size)?;
    let (file, digest) = match &descriptor.source {
        DescriptorSource::File { path, stamp } => {
            fs::snapshot(&mut fs::open(path)?.file, stamp, &mut check)?
        }
        DescriptorSource::InlineImage => {
            let references::Reference::InlineImage { url, .. } = &descriptor.selection.value else {
                return Err("内嵌图片引用无效".into());
            };
            let metadata = inline_image_metadata(url)?;
            fs::snapshot_inline_image(
                metadata.encoded,
                descriptor.size,
                descriptor.mime,
                &mut check,
            )?
        }
    };
    check()?;
    descriptor.validate_source()?;
    Ok(Snapshot {
        file,
        size: descriptor.size,
        name: descriptor.name.clone(),
        mime: descriptor.mime,
        digest,
        _budget: budget,
    })
}

pub(super) fn basename(value: &str) -> String {
    value
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("附件")
        .chars()
        .filter(|ch| !ch.is_control())
        .take(200)
        .collect()
}

pub(super) fn reference_id(
    turn: &Value,
    item: &Value,
    index: usize,
    part: usize,
    path: &str,
) -> String {
    let identity = if item.is_string() || item.is_number() {
        item.clone()
    } else {
        json!(["index", index])
    };
    crate::fs_util::sha256_hex_str(&json!([turn.as_str(), identity, part, path]).to_string())
}

pub(super) fn inline_image_reference_id(
    turn_id: &Value,
    message_id: &Value,
    image_slot: usize,
    url: &str,
) -> String {
    crate::fs_util::sha256_hex_str(
        &json!([
            "inline-image",
            turn_id,
            message_id,
            image_slot,
            crate::fs_util::sha256_hex_str(url)
        ])
        .to_string(),
    )
}

struct InlineImageMetadata<'a> {
    encoded: &'a str,
    size: u64,
    mime: &'static str,
}

fn inline_image_metadata(url: &str) -> Result<InlineImageMetadata<'_>, String> {
    let (mime, encoded) = url
        .strip_prefix("data:")
        .and_then(|url| url.split_once(";base64,"))
        .ok_or("内嵌图片格式无效")?;
    let mime = match mime {
        "image/png" => "image/png",
        "image/jpeg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        _ => return Err("不支持此内嵌图片类型".into()),
    };
    if encoded.len() > MAX_INLINE_IMAGE_ENCODED {
        return Err("内嵌图片超过下载上限".into());
    }
    if encoded.is_empty()
        || !encoded.len().is_multiple_of(4)
        || !encoded.as_bytes()[..encoded.len() - 4]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    {
        return Err("内嵌图片 Base64 编码无效".into());
    }
    // STANDARD checks final padding/trailing bits; only the format header is decoded here.
    let tail = STANDARD
        .decode_slice(&encoded.as_bytes()[encoded.len() - 4..], &mut [0; 3])
        .map_err(|_| "内嵌图片 Base64 编码无效")?;
    let mut header = [0; 12];
    let count = STANDARD
        .decode_slice(&encoded.as_bytes()[..encoded.len().min(16)], &mut header)
        .map_err(|_| "内嵌图片 Base64 编码无效")?;
    if fs::image_mime(&header[..count]) != Some(mime) {
        return Err("内嵌图片类型与内容不匹配".into());
    }
    Ok(InlineImageMetadata {
        encoded,
        size: ((encoded.len() / 4 - 1) * 3 + tail) as u64,
        mime,
    })
}

pub(super) fn public_image_url(value: &Value) -> Option<&str> {
    let url = value.as_str()?;
    if url.starts_with("https:") {
        let parsed = reqwest::Url::parse(url).ok()?;
        return (parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none())
        .then_some(url);
    }
    inline_image_metadata(url).ok().map(|_| url)
}

pub(super) fn download_route(path: &str) -> Option<(&str, &str)> {
    let path = path.strip_prefix("/api/sessions/")?;
    let (thread, id) = path.split_once("/files/")?;
    if uuid::Uuid::parse_str(thread).ok()?.to_string() != thread
        || id.len() != 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    Some((thread, id))
}

pub(super) fn disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .take(150)
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let ascii = if ascii.is_empty() || matches!(ascii.as_str(), "." | "..") {
        "download"
    } else {
        &ascii
    };
    format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{}",
        percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC)
    )
}

#[cfg(test)]
mod tests;
