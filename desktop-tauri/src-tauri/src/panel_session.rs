//! 桌面壳的面板会话（登录令牌的保存、注入与自动续期）。
//!
//! ── 这套东西解决什么 ────────────────────────────────────────
//! 桌面窗口加载 `tauri://localhost`，网关在 `127.0.0.1:{port}` —— 两者跨源，
//! 登录响应的 HttpOnly cookie 落不到网关的源上（就算落到了，窗口的页面也带不上）。
//! 所以桌面形态借用网页端「中转模式」同一条通道：登录/续期的响应体里带回
//! `accessToken` / `refreshToken`，之后每个管理请求带 `x-panel-token` 头。
//! 服务端 `access_candidates` 本来就把这条头通道排在 cookie 之后 —— 两种形态
//! 共用一套校验，这里只是把「谁来保管令牌」从 localStorage 换成了壳进程。
//!
//! ── 什么时候启用 ────────────────────────────────────────────
//! 已注册管理员时必须登录；未注册时只为可信的本机壳签发管理会话，
//! 避免受限 Key 被用于管理接口。这份会话不能代替模型调用的网关 Key。
//!
//! ── 登录流程 ────────────────────────────────────────────────
//! 已注册管理员时，窗口先加载 `login.html`（与网页端同一份页面），登录成功后
//! 页面把令牌交给壳（`panel_session_store` 命令）并跳转到主界面。壳把令牌
//! 加密后写进桌面设置（`AppSettings::panel_session`），重启后直接复用。
//!
//! ── 自动续期 ────────────────────────────────────────────────
//! access 只有 2 小时。`authorized_call` 收到 401 且响应体是
//! `panel_login_required` 时，先用 refresh 换一组新令牌、重试一次；续期也失败
//! 才把错误抛给界面（界面据此提示「登录已过期」，并把窗口送回登录页）。
//!
//! ── 为什么令牌要加密落盘 ────────────────────────────────────
//! 它是一对管理面凭据。`AppSettings` 整份是明文 JSON 进 `kv` 表，直接放进去等于
//! 「拷走数据库就接管面板」。这里用账号域同一把密钥（`account.key`）封装：
//! 能读那把钥匙的人本来就能读全部账号，信任边界不扩大。

use std::sync::Mutex;

use serde_json::Value;

use crate::gateway;
use crate::settings::{self, PanelSession};

/// 进程内的令牌缓存（避免每个请求都解密一次）。
///
/// `None` = 还没读过 / 已清空；两者都要回落到磁盘再读。`Some` 里的空串表示
/// 「确认没有令牌」，与「没读过」区分开 —— 否则每个请求都会去尝试解密一次。
static CACHE: Mutex<Option<TokenPair>> = Mutex::new(None);

#[derive(Clone, Debug, Default)]
pub struct TokenPair {
    pub access: String,
    pub refresh: String,
    /// access 的到期时刻（Unix 毫秒；0 = 不知道）。
    ///
    /// **不是凭据**：只用来判断「这个请求发出前要不要先续期」（见 [`refresh_if_expiring`]）。
    /// 算错只会退回「401 → 续期」的老路径 —— 服务端始终按自己的会话表判有效期。
    pub expires_at: i64,
}

impl TokenPair {
    fn is_empty(&self) -> bool {
        self.access.is_empty() && self.refresh.is_empty()
    }
}

/// 读令牌：缓存 → 桌面设置（解密）→ 空。
///
/// 解不开（密钥换过 / 文件丢了）按「没有令牌」处理，不报错 —— 用户重新登录一次
/// 就能自愈，而在这里报错会让每个管理请求都弹一条看不懂的提示。
fn load_tokens() -> TokenPair {
    if let Ok(guard) = CACHE.lock() {
        if let Some(pair) = guard.as_ref() {
            return pair.clone();
        }
    }
    let stored = settings::load().panel_session;
    let pair = TokenPair {
        access: open(&stored.access),
        refresh: open(&stored.refresh),
        expires_at: stored.expires_at,
    };
    if let Ok(mut guard) = CACHE.lock() {
        if let Some(cached) = guard.as_ref() {
            return cached.clone();
        }
        *guard = Some(pair.clone());
    }
    pair
}

/// 写入令牌（登录成功后由界面交来）：加密落盘 + 更新缓存。
///
/// `expires_in` 是服务端下发的剩余秒数（见 `api::panel::issue_response`）；
/// 0 / 负数表示「不知道」，那时 `expires_at` 落 0，调用方退回 401 被动续期。
pub fn store(access: &str, refresh: &str, expires_in: i64) -> Result<(), String> {
    let expires_at = expiry_from_ttl(expires_in);
    let pair = TokenPair {
        access: access.to_string(),
        refresh: refresh.to_string(),
        expires_at,
    };
    let mut current = settings::load();
    current.panel_session = PanelSession {
        access: seal(access),
        refresh: seal(refresh),
        expires_at,
    };
    settings::save(&current)?;
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(pair);
    }
    Ok(())
}

/// 剩余秒数 → 绝对到期时刻（Unix 毫秒）。`<= 0` 一律 0（= 不知道）。
fn expiry_from_ttl(expires_in: i64) -> i64 {
    if expires_in <= 0 {
        return 0;
    }
    now_ms().saturating_add(expires_in.saturating_mul(1000))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// 清空令牌（登出 / 续期彻底失败 / 改了密码后被服务端撤销）。
pub fn clear() {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(TokenPair::default());
    }
    let mut current = settings::load();
    if !current.panel_session.is_present() {
        return;
    }
    current.panel_session = PanelSession::default();
    let _ = settings::save(&current);
}

/// 是否有可尝试的令牌（不判断有效性 —— 那是服务端的事）。
pub fn has_tokens() -> bool {
    !load_tokens().is_empty()
}

/// 给请求加上会话令牌头（有令牌时）。`x-panel-token` 与 `Authorization` 一起发：
/// 后者是标准头，连自定义头都剥的中转也放它过去（与 web_shim 同口径）。
pub fn attach(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let pair = load_tokens();
    let mut builder = builder;
    if !pair.access.is_empty() {
        builder = builder
            .header("x-panel-token", &pair.access)
            .header("Authorization", format!("Bearer {}", pair.access));
    }
    if !pair.refresh.is_empty() {
        builder = builder.header("x-panel-refresh", &pair.refresh);
    }
    builder
}

pub fn ensure_local_access() -> Result<(), String> {
    if crate::server::access::admin_registered() {
        return Ok(());
    }
    let pair = load_tokens();
    if !pair.access.is_empty() && pair.expires_at > now_ms() {
        return Ok(());
    }
    // 未注册管理员时，本机壳本就可信；这份会话只准入管理面，不能代替模型 Key。
    let issued = crate::server::access::IssuedSession::new_session();
    store(&issued.access_token, &issued.refresh_token, issued.expires_in)
}

pub fn credential_headers() -> serde_json::Map<String, Value> {
    let pair = load_tokens();
    let mut headers = serde_json::Map::new();
    if !pair.access.is_empty() {
        headers.insert("x-panel-token".to_string(), Value::String(pair.access));
    }
    if !pair.refresh.is_empty() {
        headers.insert("x-panel-refresh".to_string(), Value::String(pair.refresh));
    }
    headers
}

/// 用 refresh 换一组新令牌。成功返回 true。
///
/// 直接打 `/api/panel/refresh?auth-mode=body`：query 标记让服务端把新令牌放进
/// 响应体（桌面窗口没有 cookie 通道，只能走 body）。新令牌**必须**落盘 ——
/// 服务端的轮换会让旧 refresh 立即作废，不保存就等于下次续期必然失败。
pub async fn refresh() -> bool {
    if crate::startup::wait().await.is_err() {
        return false;
    }
    let pair = load_tokens();
    refresh_pair(pair).await
}

/// 同一进程的并发接口共用一次令牌轮换，不能让晚到的失败清掉新会话。
async fn refresh_pair(expected: TokenPair) -> bool {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = LOCK.lock().await;
    let pair = load_tokens();
    if pair.access != expected.access || pair.refresh != expected.refresh {
        return !pair.is_empty();
    }
    if pair.refresh.is_empty() {
        return false;
    }
    let url = format!(
        "http://127.0.0.1:{}/api/panel/refresh?auth-mode=body",
        gateway::proxy_port()
    );
    let client = match gateway::local_client(std::time::Duration::from_millis(
        gateway::REQUEST_TIMEOUT_MS,
    )) {
        Ok(client) => client,
        Err(_) => return false,
    };
    let response = client
        .post(&url)
        .header("x-panel-refresh", &pair.refresh)
        .header("x-panel-auth-mode", "body")
        .send()
        .await;
    let Ok(response) = response else { return false };
    if response.status().as_u16() == 401 {
        let current = load_tokens();
        if current.access == pair.access && current.refresh == pair.refresh {
            clear();
        }
        return false;
    }
    if !response.status().is_success() {
        return false;
    }
    let Ok(text) = response.text().await else { return false };
    let Ok(payload) = serde_json::from_str::<Value>(&text) else { return false };
    let data = payload.get("data").unwrap_or(&Value::Null);
    let access = data.get("accessToken").and_then(Value::as_str).unwrap_or("");
    let refresh = data.get("refreshToken").and_then(Value::as_str).unwrap_or("");
    if access.is_empty() || refresh.is_empty() {
        return false;
    }
    let expires_in = data.get("expiresIn").and_then(Value::as_i64).unwrap_or(0);
    let current = load_tokens();
    if current.access != pair.access || current.refresh != pair.refresh {
        return !current.is_empty();
    }
    store(access, refresh, expires_in).is_ok()
}

/// 主动续期的提前量（毫秒）：到期前这么久就把令牌换掉。
///
/// 两分钟够覆盖「这一轮请求刚好在到期瞬间发出」的抖动，又远小于 2 小时的寿命 ——
/// 不会因为一次慢请求就提前换掉刚签发的令牌（那会白耗一次轮换）。
const REFRESH_AHEAD_MS: i64 = 2 * 60 * 1000;

/// 到期前主动续期：把「access 过期 → 401 → 续期」提前到过期之前。
///
/// 只靠 401 被动续期时，**每次** access 过期都必然产生一条 401 —— 桌面端一样
/// 会撞上（壳里那些后台轮询每 20 秒一轮），于是服务端每 2 小时准时记一条。
/// 服务端已按「refresh 还活着」把这类 401 降成 verbose；这里再从源头把请求
/// 本身消掉。
///
/// 失败一律静默放行：可能只是网络抖动，真正的判据（会话还有效吗）只有服务端
/// 知道，交回给 `gateway::call` 里既有的 401 分支处理。
pub async fn refresh_if_expiring() {
    if crate::startup::wait().await.is_err() {
        return;
    }
    let pair = load_tokens();
    if pair.expires_at <= 0 || pair.refresh.is_empty() {
        return;
    }
    if now_ms() < pair.expires_at.saturating_sub(REFRESH_AHEAD_MS) {
        return;
    }
    refresh_pair(pair).await;
}

// ── 加解密（账号域密钥；失败一律当作「没有」）────────────────

fn seal(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    agent2api_server::server::core::crypto::seal(
        plain.as_bytes(),
        agent2api_server::server::core::crypto::SecretScope::Accounts,
    )
    .unwrap_or_default()
}

fn open(stored: &str) -> String {
    if stored.is_empty() {
        return String::new();
    }
    agent2api_server::server::core::crypto::open(
        stored,
        agent2api_server::server::core::crypto::SecretScope::Accounts,
    )
    .ok()
    .and_then(|bytes| String::from_utf8(bytes).ok())
    .unwrap_or_default()
}
