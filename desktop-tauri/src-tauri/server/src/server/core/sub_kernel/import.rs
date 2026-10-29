//! 手动内核安装与下载共用校验值；路径来源先复制到私有暂存位，安装不改动原文件。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use super::{assets, kernel_binary_name, kernel_dir, layout, runtime, ProviderInput, KERNEL_VERSION};

pub const MAX_FILE_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_BASE64_BYTES: usize = (MAX_FILE_BYTES + 2) / 3 * 4;
pub const MAX_UPLOAD_BYTES: usize = MAX_BASE64_BYTES + 1024 * 1024;
static INSTALLING: AtomicBool = AtomicBool::new(false);

pub struct InstallPermit;

impl Drop for InstallPermit {
    fn drop(&mut self) { INSTALLING.store(false, Ordering::SeqCst); }
}

pub fn reserve() -> Result<InstallPermit, String> {
    INSTALLING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .map(|_| InstallPermit)
        .map_err(|_| "内核正在下载或导入，请等待当前操作完成".to_string())
}

pub fn busy() -> bool { INSTALLING.load(Ordering::SeqCst) }

pub enum Source {
    Upload(Vec<u8>),
    Path(PathBuf),
}

pub(super) struct Job {
    pub source: Source,
    pub dir: PathBuf,
    pub replace: bool,
    pub providers: Vec<ProviderInput>,
    pub permit: InstallPermit,
}

pub(super) struct Completion {
    pub saved: bool,
    pub start_error: Option<String>,
}

struct InputFile(PathBuf);

impl Drop for InputFile {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

pub(super) async fn run(source: Source, dir: PathBuf, replace: bool, providers: Vec<ProviderInput>) -> Result<Completion, String> {
    if super::installed() && !replace {
        return Err("已有内核安装，请确认替换后再导入；订阅与账号引用会保留".to_string());
    }
    if !replace && (dir.join("current").exists() || dir.join("versions").join(KERNEL_VERSION).exists()) {
        return Err("目标目录已有内核文件，请先确认替换，或选择新的存放目录".to_string());
    }
    super::import_phase("checking");
    let destination = dir.clone();
    let prepared = tokio::task::spawn_blocking(move || prepare(source, &destination))
        .await.map_err(|error| format!("内核校验任务失败: {error}"))??;
    let previous_dir = kernel_dir();
    let was_running = super::status_json().get("running").and_then(serde_json::Value::as_bool) == Some(true);
    let halted = super::is_manually_halted();
    let previous = previous_dir.clone();
    let destination = dir.clone();
    let provider_ids: Vec<String> = providers.iter().map(|provider| provider.id.clone()).collect();
    tokio::task::spawn_blocking(move || preserve_runtime(&previous, &destination, &provider_ids))
        .await.map_err(|error| format!("准备内核运行目录失败: {error}"))??;
    super::import_phase("installing");
    runtime::cleanup_stale_kernel().await;
    runtime::stop_kernel().await;
    let installed = tokio::task::spawn_blocking(move || layout::commit_install(prepared))
        .await.map_err(|error| format!("内核安装任务失败: {error}"))
        .and_then(|result| result);
    if let Err(error) = installed {
        if was_running {
            if let Err(restart_error) = runtime::run_restart(&providers).await {
                return Err(format!("{error}；原内核重新启动也失败：{restart_error}"));
            }
        }
        return Err(error);
    }
    let configured = if dir == super::default_kernel_dir() { None } else { Some(dir.to_string_lossy().to_string()) };
    let saved = crate::server::config::set_kernel_dir(configured);
    super::reset_runtime_snapshot();
    super::log_line(&format!("📦 代理内核 {KERNEL_VERSION} 已手动导入到 {}", dir.display()));
    let start_error = if !providers.is_empty() && !halted {
        super::import_phase("starting");
        runtime::run_restart(&providers).await.err()
    } else { None };
    if let Some(error) = &start_error {
        super::log_line(&format!("❌ 内核已导入，但启动失败: {error}"));
    }
    Ok(Completion { saved, start_error })
}

fn prepare(source: Source, dir: &Path) -> Result<layout::PreparedInstall, String> {
    let asset = assets::current().ok_or_else(|| "当前平台暂不支持代理内核".to_string())?;
    let staging = crate::paths::config_dir().join("kernel-imports")
        .join(format!(".import-{}-{}", std::process::id(), crate::server::logging::now_ms()));
    std::fs::create_dir_all(staging.parent().ok_or_else(|| "内核暂存目录无效".to_string())?)
        .map_err(|error| format!("创建内核暂存目录失败: {error}"))?;
    std::fs::create_dir(&staging).map_err(|error| format!("创建内核暂存目录失败: {error}"))?;
    let temporary = InputFile(staging);
    let input = temporary.0.join("input");
    match source {
        Source::Upload(bytes) => {
            if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES { return Err(size_error()); }
            std::fs::write(&input, bytes).map_err(|error| format!("写入内核导入文件失败: {error}"))?;
        }
        Source::Path(path) => {
            let path = source_file(&path, asset.name)?;
            let mut source = std::fs::File::open(&path).map_err(|error| format!("打开内核文件失败（{}）: {error}", path.display()))?;
            let metadata = source.metadata().map_err(|error| format!("读取内核文件信息失败: {error}"))?;
            if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_FILE_BYTES as u64 { return Err(size_error()); }
            let mut output = std::fs::File::create(&input).map_err(|error| format!("创建内核导入文件失败: {error}"))?;
            let copied = std::io::copy(&mut Read::by_ref(&mut source).take(MAX_FILE_BYTES as u64 + 1), &mut output)
                .map_err(|error| format!("读取内核文件失败: {error}"))?;
            output.flush().map_err(|error| format!("写入内核导入文件失败: {error}"))?;
            if copied == 0 || copied > MAX_FILE_BYTES as u64 { return Err(size_error()); }
        }
    }
    let digest = sha256_file(&input)?;
    let archive = if digest == asset.archive_sha256 { true }
        else if digest == asset.binary_sha256 { false }
        else { return Err(format!("内核文件 SHA-256 校验失败：仅接受官方 {} 或其解压后的二进制；请检查平台、版本与文件是否完整，原安装未修改", asset.name)); };
    layout::prepare_install_at(dir, &input, KERNEL_VERSION, archive)
}

fn source_file(path: &Path, asset: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() { return Err("内核文件或目录必须是网关机器上的绝对路径".to_string()); }
    if !path.exists() { return Err(format!("网关机器上找不到该路径：{}；远程面板不能读取浏览器电脑的路径，请上传文件", path.display())); }
    if !path.is_dir() { return Ok(path.to_path_buf()); }
    let stem = asset.trim_end_matches(".zip").trim_end_matches(".gz");
    let suffix = format!("-{KERNEL_VERSION}");
    let stem = stem.strip_suffix(&suffix).unwrap_or(stem);
    let unpacked = if cfg!(windows) { format!("{stem}.exe") } else { stem.to_string() };
    let found = [asset, kernel_binary_name(), unpacked.as_str()].into_iter()
        .map(|name| path.join(name)).find(|file| file.is_file());
    found.ok_or_else(|| format!("目录里没有 {asset} 或 {}；可填写完整文件路径", kernel_binary_name()))
}

fn preserve_runtime(from: &Path, to: &Path, provider_ids: &[String]) -> Result<(), String> {
    if same_directory(from, to) { return Ok(()); }
    std::fs::create_dir_all(to).map_err(|error| format!("创建内核存放目录失败: {error}"))?;
    let state = from.join("kernel-state.json");
    let destination = to.join("kernel-state.json");
    if state.is_file() && !destination.exists() {
        let bytes = std::fs::read(&state).map_err(|error| format!("读取原内核端口映射失败: {error}"))?;
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| format!("原内核端口映射无效: {error}"))?;
        if let Some(object) = value.as_object_mut() { object.remove("pid"); }
        let text = serde_json::to_vec(&value).map_err(|error| format!("编码内核端口映射失败: {error}"))?;
        std::fs::write(&destination, text).map_err(|error| format!("保留内核端口映射失败: {error}"))?;
    }
    for id in provider_ids {
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_') {
            return Err("订阅 id 无效".to_string());
        }
        let name = format!("sub_{id}.yaml");
        let source = from.join("providers").join(&name);
        let target = to.join("providers").join(&name);
        if source.is_file() && !target.exists() {
            std::fs::create_dir_all(to.join("providers")).map_err(|error| format!("创建订阅缓存目录失败: {error}"))?;
            std::fs::copy(source, target).map_err(|error| format!("保留订阅缓存失败: {error}"))?;
        }
    }
    Ok(())
}

fn same_directory(first: &Path, second: &Path) -> bool {
    let first = std::fs::canonicalize(first).unwrap_or_else(|_| first.to_path_buf());
    let second = std::fs::canonicalize(second).unwrap_or_else(|_| second.to_path_buf());
    first == second
}

pub(super) fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| format!("读取内核校验文件失败: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| format!("读取内核校验文件失败: {error}"))?;
        if count == 0 { break; }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn size_error() -> String {
    format!("内核文件必须为非空且不超过 {} MiB 的官方压缩包或二进制", MAX_FILE_BYTES / 1024 / 1024)
}
