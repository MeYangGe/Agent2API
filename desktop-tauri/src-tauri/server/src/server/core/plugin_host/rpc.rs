//! `RpcAdapter`：把 `ProviderAdapter` 的每个方法变成一次 RPC。
//!
//! ── 这一层为什么必须存在 ────────────────────────────────────
//! 上层（转发循环、目录聚合、账号 API）只认识
//! [`ProviderAdapter`](crate::server::core::providers::adapter::ProviderAdapter)。
//! 改成插件架构之后，那个 trait 的实现体不能是本进程的 Rust 代码，而必须
//! 是「把调用转发给子进程」。`RpcAdapter` 就是那个实现 —— 一个 provider
//! 一个实例，持有宿主的 `Arc` 与自己的 id。
//!
//! ── 能力位为什么是本地字段 ──────────────────────────────────
//! `supports_*` 这类方法在热路径上被反复调用（`sse_model_rewrite` 每一帧
//! 都要判）。它们必须在 `init` 时一次性取回并缓存在本地 —— 每次走 RPC
//! 会把流式转发变成「每帧一次往返」。这也是协议里把能力位放握手的原因。
//!
//! ── 哪些方法走 RPC、哪些不走 ────────────────────────────────
//! 走 RPC：所有需要 provider 知识且不在热路径上的（构造请求、分类、
//! 凭证、目录、余额、签到、账号规范化、登录）。
//! 不走 RPC：能力位（本地缓存）、`kind`（本地字段）。
//!
//! ── 热路径上的一个例外：`build_chat_request` ─────────────────
//! 它每个请求走一次 RPC。这是可接受的：一次 RPC 的往返在本机管道上约
//! 几十微秒，而上游请求本身是几百毫秒到几十秒。真正的热路径是 SSE 逐帧
//! 下发，那条路在 `conversation` 里由插件自己推、网关只读帧。

use std::sync::Arc;
use std::time::Duration;

use agent2api_provider_protocol as proto;
use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, ReasoningPatch, RetryAdvice,
    RetryDecision, UpstreamErrorClass, UpstreamResponse,
};
use crate::server::core::providers::ProviderKind;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::ForwardOutcome;
use crate::server::errors::GatewayError;

use super::Host;

/// 一次 RPC 调用的超时。
///
/// ── 为什么不用一个统一值 ────────────────────────────────────
/// 控制面（能力位、目录、账号）应当快速失败；而 `refresh_models` 要真打
/// 上游、`login.poll` 要等用户操作，它们可能是几十秒。分档见各调用点。
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const NETWORK_TIMEOUT: Duration = Duration::from_secs(120);

/// 宿主启动参数。
#[derive(Clone, Debug)]
pub struct HostOptions {
    /// 入口可执行文件。
    pub exe: std::path::PathBuf,
    /// 额外命令行参数。
    pub args: Vec<String>,
    /// 工作目录。
    pub cwd: Option<std::path::PathBuf>,
    /// 数据目录（插件可写自己的缓存）。
    pub data_dir: String,
    /// 网关版本。
    pub host_version: String,
    /// 目标三元组。
    pub target: String,
    /// 网关默认出口。
    pub proxy: String,
    /// 调试模式。
    pub debug_traffic: bool,
    /// 握手超时（插件可能要做一次性的模型目录同步）。
    pub init_timeout: Duration,
    /// 网关监听的 HTTP 端口（`0` = 未知）。
    ///
    /// ── 为什么由宿主告诉插件 ────────────────────────────────────
    /// 回调型网页登录的 `redirect_uri` 要指向网关自己的 loopback 地址，
    /// 而那是**宿主**才知道的事实（见 `HostEnv.loopback_port`）。
    pub loopback_port: u16,
}

impl HostOptions {
    /// 最简构造。
    pub fn new(exe: impl Into<std::path::PathBuf>) -> Self {
        Self {
            exe: exe.into(),
            args: Vec::new(),
            cwd: None,
            data_dir: String::new(),
            host_version: env!("CARGO_PKG_VERSION").to_string(),
            target: target_triple(),
            proxy: String::new(),
            debug_traffic: false,
            init_timeout: Duration::from_secs(60),
            loopback_port: 0,
        }
    }
}

/// 当前编译目标三元组（运行时拼，避免引入 `build.rs`）。
///
/// ── 为什么是完整的 Rust 三元组，而不是 `arch-os` ─────────────
/// 这个值有四个消费方，它们都必须用**同一个字符串**：
///   1. `HostEnv.target`（插件侧的运行环境）；
///   2. `pack.json` 的 `entry` 键查找；
///   3. 签名载荷里的 `target` 项（见 `proto::signing_payload`）；
///   4. 插件更新时拼资产名（`scripts/pack.mjs`（Agent2Providers 仓库） 产出的是
///      `agent2api-providers-<ver>-<三元组>.zip`）。
///
/// 早期这里返回 `x86_64-windows` 这种 `arch-os` 形态，靠 `entry_for_target`
/// 的「包含 arch 且包含 os」宽松匹配歪打正着能用 —— 但签名是**逐字比对**，
/// 宽松匹配救不了它（症状是「签名包装上去显示未签名」，不报错、很难查）。
/// 所以改成标准三元组，让四处逐字一致。
pub(super) fn target_triple() -> String {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        // Windows 的 GNU ABI 我们也发（CI 只出 msvc 包，但保留分支让将来可加）
        "windows" => format!("{arch}-pc-windows-msvc"),
        "macos" => format!("{arch}-apple-darwin"),
        // Linux / 其它一律按 gnu ABI：与 `pack.mjs` 的默认分支一致
        _ => format!("{arch}-unknown-linux-gnu"),
    }
}

/// 一个远程 provider。
///
/// ── 生命周期 ────────────────────────────────────────────────
/// 与宿主同寿（宿主重启后这些实例仍然有效 —— 它们只持有 `Arc<Host>` 与
/// id，方法调用时宿主会自行确保进程在跑）。
pub struct RpcAdapter {
    /// 这个实例对应的 provider id（leak 成 `'static`，见 `provider_id()`）。
    id: &'static str,
    /// 展示名（握手带回，同上）。
    label: &'static str,
    /// provider id 对应的 `ProviderKind`（网关内部仍用它做若干穷举判断）。
    ///
    /// ── 为什么还留着 `ProviderKind` ─────────────────────────────
    /// 网关里有 370 处 `ProviderKind::` 引用（账号分派、签到清单、
    /// 模型规则种子……）。把它们全部改成字符串比较是 P6 的事；在那之前，
    /// 内置 provider 仍然有对应的枚举值，而**第三方新增的 provider 没有** ——
    /// 见 [`Self::kind`] 的说明。
    kind: ProviderKind,
    /// 宿主。
    host: Arc<Host>,
    /// 握手时取回的能力位（本地缓存）。
    capabilities: proto::Capabilities,
    image_generation: bool,
    image_editing: bool,
    image_models: Vec<Value>,
    /// `finish_reason` 拒答集合（握手时取回）。
    ///
    /// ── 为什么存的是 `&'static [&'static str]` 而不是 `Box<[...]>` ──
    /// trait 的签名要求返回 `&'static [...]`。存 `Box<[...]>` 只能借出
    /// `&'a [...]`（跟着 `&self` 走），不满足 `'static`。所以整个切片也要
    /// leak：`Box::leak` 得到 `&'static mut [T]`，转成 `&'static [T]` 即可。
    /// 一个适配器只做一次，几十字节。
    refusal_finishes: &'static [&'static str],
    /// 正文里的配额耗尽标记（同上）。
    refusal_markers: &'static [&'static str],
}

impl RpcAdapter {
    pub fn new(
        id: String,
        kind: ProviderKind,
        host: Arc<Host>,
        info: proto::ProviderInfo,
    ) -> Self {
        Self {
            id: leak_str(id),
            label: leak_str(info.label.clone()),
            kind,
            host,
            capabilities: info.capabilities,
            image_generation: false,
            image_editing: false,
            image_models: info.default_models.into_iter()
                .filter(crate::server::core::providers::images::model_is_image).collect(),
            refusal_finishes: leak_slice(info.refusal_finishes),
            refusal_markers: leak_slice(info.refusal_markers),
        }
    }

    pub(super) async fn load_image_capability(&mut self) {
        let result = self.control(
            crate::server::core::providers::images::CAPABILITIES_METHOD,
            Value::Object(self.base()),
        ).await;
        let capabilities = result.unwrap_or(Value::Null);
        self.image_generation = capabilities.get("generation").and_then(Value::as_bool).unwrap_or(false);
        self.image_editing = capabilities.get("editing").and_then(Value::as_bool).unwrap_or(false);
        if !self.image_generation {
            self.image_models.clear();
        }
    }

    /// 调一次控制面 RPC。
    async fn control(&self, method: &str, params: Value) -> Result<Value, GatewayError> {
        self.call(method, params, CONTROL_TIMEOUT).await
    }

    /// 调一次要出网的 RPC（超时更宽）。
    async fn network(&self, method: &str, params: Value) -> Result<Value, GatewayError> {
        self.call(method, params, NETWORK_TIMEOUT).await
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, GatewayError> {
        // 每次调用都先 ensure：崩溃之后第一次调用会重新拉起子进程。
        self.host
            .ensure()
            .await
            .map_err(|e| GatewayError::with_status(503, format!("提供商插件不可用：{e}")))?;
        self.host.call_coded(method, params, timeout).await.map_err(|error| {
            if let Some(failure) = super::errors::control_error(&error) {
                return failure;
            }
            // ── 「本地前置条件不成立」要还原成 556 而不是 502 ────────────
            // 编排层对这个码的处置与别的错误**完全不同**：它意味着请求还没
            // 出本机就注定发不出去（今天的用户：ZCode 活动套餐通道缺人机
            // 验证令牌，而本机没有 WebView 能铸），此时该换下一个候选家
            // 顺延、选路暂时绕开这一家 —— 而不是终止整份请求。
            // 折成 502 的后果是「一家本地发不出去 = 整条请求失败」，生产上
            // 留下的形状是 `attempts=1`、别家明明能答。
            if error.code.as_deref() == Some(proto::WireError::LOCAL_PRECONDITION) {
                return GatewayError::with_status(
                    i32::from(super::super::providers::readiness::LOCAL_PRECONDITION_STATUS),
                    error.message,
                );
            }
            let status = if matches!(
                error.code.as_deref(),
                Some(proto::WireError::NO_METHOD) | Some(proto::WireError::UNSUPPORTED)
            ) {
                501
            } else {
                502
            };
            let mut failure = GatewayError::with_status(
                status,
                format!("提供商插件调用失败（{method}）：{}", error.message),
            );
            failure.code = error.code;
            failure
        })
    }

    /// 本 provider 的参数前缀（每个方法都要带 provider）。
    fn base(&self) -> serde_json::Map<String, Value> {
        let mut map = serde_json::Map::new();
        map.insert("provider".into(), Value::String(self.id.to_string()));
        let timeouts = crate::server::config::timeout_settings();
        map.insert("timeouts".into(), serde_json::json!({
            "connectSeconds": timeouts.connect_seconds,
            "headersSeconds": timeouts.headers_seconds,
            "streamIdleSeconds": timeouts.stream_idle_seconds,
            "bodySeconds": timeouts.body_seconds,
        }));
        map
    }

    /// 带 account 的参数。
    fn with_account(&self, account: &str) -> Value {
        let mut map = self.base();
        map.insert("account".into(), Value::String(account.to_string()));
        Value::Object(map)
    }
}

impl ProviderAdapter for RpcAdapter {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn provider_id(&self) -> &'static str {
        self.id
    }

    fn label(&self) -> &'static str {
        self.label
    }

    fn list_models(&self) -> Vec<Value> {
        // ── 为什么这里是同步的 ──────────────────────────────────
        // trait 的 `list_models` 是同步方法（改造前的实现都只读本地缓存）。
        // 跨进程拿清单只能异步，因此这里**只返回本地缓存**，由后台任务
        // 定期调 `models.list` 填充。缓存为空时返回内置兜底（握手时
        // `provider.info` 已经带回来了）。
        let mut models = super::rpc_cache::models_for(self.id).unwrap_or_default();
        for image in &self.image_models {
            let id = crate::server::core::models::model_id(image);
            if let Some(existing) = models.iter_mut().find(|item| crate::server::core::models::model_id(item).eq_ignore_ascii_case(&id)) {
                existing["kind"] = serde_json::json!("image");
                existing["output_modalities"] = image.get("output_modalities").cloned().unwrap_or(serde_json::json!(["image"]));
                existing["supportsToolCall"] = Value::Bool(false);
                existing["isDefault"] = Value::Bool(false);
            } else if image.get("synthetic").and_then(Value::as_bool) == Some(true) {
                models.push(image.clone());
            }
        }
        models
    }

    fn stream_refusal_finishes(&self) -> &'static [&'static str] {
        self.refusal_finishes
    }

    fn stream_refusal_markers(&self) -> &'static [&'static str] {
        self.refusal_markers
    }

    fn build_chat_request_async<'a>(
        &'a self,
        account: &'a Value,
        body: &'a Value,
        client_headers: &'a HeaderMap,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ChatRequestPlan, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut params = self.base();
            params.insert("account".into(), Value::String(account_id_of(account)));
            params.insert("proxy".into(), proxy_of(account));
            params.insert("body".into(), body.clone());
            params.insert("headers".into(), headers_json(client_headers));
            params.insert("stream".into(), Value::Bool(true));
            // 发送名与思考档位由调用方在 body 里已经改好（`upstream::payload`
            // 的模型改写发生在调这个方法**之前**），所以这里从 body 里读。
            let wire_model = body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            params.insert("wire_model".into(), Value::String(wire_model));
            params.insert("reasoning".into(), Value::String(String::new()));

            let value = self.network(proto::methods::gateway::CHAT, Value::Object(params)).await?;
            let url = value.get("url").and_then(Value::as_str).unwrap_or("").to_string();
            if url.is_empty() {
                return Err(GatewayError::with_status(502, "插件没有给出上游地址"));
            }
            let headers = value
                .get("headers")
                .and_then(|v| serde_json::from_value::<Vec<(String, String)>>(v.clone()).ok())
                .unwrap_or_default();
            let mut body = value.get("body").cloned().unwrap_or(Value::Null);
            let upstream = value
                .get("upstream")
                .and_then(|v| serde_json::from_value::<proto::UpstreamKind>(v.clone()).ok())
                .unwrap_or_default();
            if upstream == proto::UpstreamKind::Gemini {
                super::gemini_signatures::fill_cached(&mut body);
            }
            Ok(ChatRequestPlan {
                url,
                headers,
                body,
                response: upstream_response(upstream),
            })
        })
    }

    fn supports_image_generation(&self) -> bool {
        self.image_generation
    }

    fn supports_image_editing(&self) -> bool {
        self.image_editing
    }

    fn build_image_request_async<'a>(
        &'a self,
        account: &'a Value,
        body: &'a Value,
        client_headers: &'a HeaderMap,
        operation: crate::server::core::providers::images::ImageOperation,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ChatRequestPlan, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            use crate::server::core::providers::images::ImageRequestPlan;
            let params = serde_json::json!({
                "provider": self.id,
                "account": account_id_of(account),
                "proxy": proxy_of(account),
                "body": body,
                "headers": headers_json(client_headers),
                "wire_model": body.get("model").and_then(Value::as_str).unwrap_or(""),
            });
            // RPC 一帧上限 16 MiB；超限在写管道前拒绝，不让插件因读到半帧退出。
            let size = serde_json::to_vec(&params).map_err(|_| GatewayError::bad_request("图片请求无法序列化"))?.len();
            if size > 14 * 1024 * 1024 {
                return Err(GatewayError::with_status(413, "图片请求内联数据总大小不得超过 14 MiB"));
            }
            let value = self.network(operation.method(), params).await?;
            let plan: ImageRequestPlan = serde_json::from_value(value)
                .map_err(|error| GatewayError::with_status(502, format!("插件生图计划无法解析：{error}")))?;
            if plan.url.trim().is_empty() || !plan.body.is_object() {
                return Err(GatewayError::with_status(502, "插件生图计划缺少上游地址或 JSON 请求体"));
            }
            Ok(ChatRequestPlan {
                url: plan.url,
                headers: plan.headers,
                body: plan.body,
                response: UpstreamResponse::Images(plan.response),
            })
        })
    }

    fn is_error_response_async<'a>(
        &'a self,
        status: u16,
        headers: &'a HeaderMap,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, GatewayError>> + Send + 'a>> {
        Box::pin(async move {
            if !(200..300).contains(&status) {
                return Ok(true);
            }
            let mut params = self.base();
            params.insert("status".into(), Value::from(status));
            params.insert("headers".into(), headers_json(headers));
            let value = self.control(
                proto::methods::gateway::IS_ERROR_RESPONSE, Value::Object(params),
            ).await?;
            value.as_bool().ok_or_else(|| GatewayError::with_status(
                502, "插件返回的响应类型判定不是布尔值",
            ))
        })
    }

    fn classify_error_async<'a>(
        &'a self,
        status: u16,
        error_body: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = UpstreamErrorClass> + Send + 'a>> {
        Box::pin(async move {
            let mut params = self.base();
            params.insert("status".into(), Value::from(status));
            params.insert("body".into(), error_body.clone());
            let value = match self
                .control(proto::methods::gateway::CLASSIFY, Value::Object(params))
                .await
            {
                Ok(value) => value,
                Err(e) => {
                    // 分类拿不到 = 插件挂了。返回 Fatal 是安全兜底：不罚账号、
                    // 不重试，只把错误透传 —— 与「未覆写的家」同一处置。
                    return UpstreamErrorClass::Fatal {
                        status,
                        message: format!("提供商插件无法分类错误：{}", e.message),
                        upstream_code: None,
                    };
                }
            };
            match serde_json::from_value::<proto::ErrorClass>(value) {
                Ok(class) => upstream_error_class(class),
                Err(e) => UpstreamErrorClass::Fatal {
                    status,
                    message: format!("插件返回的错误分类无法解析：{e}"),
                    upstream_code: None,
                },
            }
        })
    }

    fn reasoning_patch_async<'a>(
        &'a self,
        level: &'a str,
        model: &'a str,
        body: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ReasoningPatch> + Send + 'a>> {
        Box::pin(async move {
            let mut params = self.base();
            params.insert("level".into(), Value::String(level.to_string()));
            params.insert("model".into(), Value::String(model.to_string()));
            params.insert("body".into(), body.clone());
            let value = match self
                .control(proto::methods::gateway::REASONING_PATCH, Value::Object(params))
                .await
            {
                Ok(value) => value,
                // 问不到就不注入 —— 与「这家不支持」同一处置，body 一个字节不动。
                Err(_) => {
                    return ReasoningPatch::Skip { reason: "提供商插件未响应，跳过档位注入" }
                }
            };
            match serde_json::from_value::<proto::ReasoningPatch>(value) {
                Ok(proto::ReasoningPatch::Set { field, value }) => ReasoningPatch::Set {
                    field: leak_str(field),
                    value,
                },
                Ok(proto::ReasoningPatch::Skip { reason }) => {
                    ReasoningPatch::Skip { reason: leak_str(reason) }
                }
                Err(_) => ReasoningPatch::Skip { reason: "插件返回的档位结果无法解析" },
            }
        })
    }

    fn credentials_expiring_async<'a>(
        &'a self,
        _store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            self.control(
                proto::methods::gateway::TOKEN_EXPIRING,
                self.with_account(account_id),
            )
            .await
            .ok()
            .and_then(|value| value.as_bool())
            // 判不出来就 false：宁可漏刷，也不要凭猜测打上游。
            .unwrap_or(false)
        })
    }

    fn build_chat_request(
        &self,
        _account: &Value,
        _body: &Value,
        _client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        // ── 同步版在插件实现里必须报错 ──────────────────────────────
        // 真正的实现在 [`Self::build_chat_request_async`]。编排层一律调异步版；
        // 走到这里说明某条调用链还在用同步签名 —— 那是接线遗漏，报错比
        // 返回一份与异步版不同的结果安全（两份分叉不会有人发现）。
        Err(GatewayError::with_status(
            501,
            format!(
                "提供商 {} 是插件实现，必须走 build_chat_request_async（同步签名调用链未切换）",
                self.id
            ),
        ))
    }

    fn classify_error(&self, status: u16, _error_body: &Value) -> UpstreamErrorClass {
        // 同步版报 Fatal（见 [`Self::classify_error_async`]）。Fatal 不罚账号、
        // 不重试 —— 即使某条链路漏改，后果也只是「这条错误没触发换号」，
        // 而不是「账号被误冷却」。
        UpstreamErrorClass::Fatal {
            status,
            message: String::new(),
            upstream_code: None,
        }
    }

    fn supports_chat(&self) -> bool {
        self.capabilities.chat
    }

    fn is_stateful(&self) -> bool {
        self.capabilities.stateful
    }

    fn supports_refresh(&self) -> bool {
        self.capabilities.refresh
    }

    fn supports_model_refresh(&self) -> bool {
        self.capabilities.model_refresh
    }

    fn supports_usage(&self) -> bool {
        self.capabilities.usage
    }

    fn supports_web_login(&self) -> bool {
        self.capabilities.web_login
    }

    fn sse_model_rewrite(&self) -> bool {
        self.capabilities.sse_model_rewrite
    }

    fn sse_strip_newline_chunks(&self) -> bool {
        self.capabilities.sse_strip_newline_chunks
    }

    fn allows_anonymous_default_session(&self) -> bool {
        self.capabilities.anonymous_default_session
    }

    fn system_proxy_when_unset(&self) -> bool {
        self.capabilities.system_proxy_when_unset
    }

    fn refresh_uses_account(&self) -> bool {
        self.capabilities.refresh_uses_account
    }

    /// 认不认「默认模型」这个概念（协议 `Capabilities.supports_default_model`）。
    ///
    /// ── 为什么必须显式接上（不能落默认 false）───────────────────
    /// 网关只在「默认值命中的 provider 里有家认这个概念」时才注入
    /// `config.defaultModel`。Accio / AtomCode / LobsterAI / Loomy / Raccoon /
    /// Qoder 六家内置实现都覆写为 true；插件侧若落默认 false，用户配的默认
    /// 模型在这六家上会**静默失效**（请求按「未指定模型」发出去，上游用它自己
    /// 的默认值，界面上看不出任何异常）。
    fn supports_default_model(&self) -> bool {
        self.capabilities.supports_default_model
    }

    /// 有没有「环境变量 / 默认登录态」旁路（协议 `Capabilities.env_credentials`）。
    ///
    /// ── 与内置实现的一处语义差（如实记录）──────────────────────
    /// 内置实现（WorkBuddy / CatPaw / AutoClaw / Raccoon）读的是**网关进程**的
    /// 环境变量与桌面端登录态路径 —— 那是「此刻真有凭证」。插件读不到那些，
    /// 只能申报「实现里有这条旁路」。消费方 `catalog::provider_available` 因此
    /// 会多广告一家「有旁路但此刻可能没凭证」的家；后果是客户端可能选中它拿到
    /// 401，比「明明有旁路却完全不广告」轻。
    fn env_credentials_present(&self) -> bool {
        self.capabilities.env_credentials
    }

    /// 插件家的签到能力位来自握手（内置家那个方法返回 `None`）。
    fn plugin_checkin_capability(&self) -> Option<bool> {
        Some(self.capabilities.checkin)
    }

    /// 厂商专属管理动作：转发成一次 `admin.action` RPC。
    ///
    /// ── 超时用 `network` 而不是 `control` ───────────────────────
    /// 这些动作全部要打上游（任务树查询、活动领取、套餐激活），control 档的
    /// 短超时会把正常的慢请求掐断。
    fn admin_action<'a>(
        &'a self,
        _store: &'a AccountStore,
        account_id: &'a str,
        action: &'a str,
        payload: &'a Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let activity = self.id == "workbuddy-intl" && action == "activity.run"
                && payload.get("mode").and_then(Value::as_str) != Some("claim");
            let mut activity_payload = payload.clone();
            let access = if activity {
                let (models, identity) = crate::server::core::billing::keepalive::authorized_models()?;
                activity_payload["models"] = serde_json::json!(models);
                Some(identity)
            } else { None };
            let mut params = self.with_account(account_id);
            if let Some(object) = params.as_object_mut() {
                object.insert("action".into(), Value::String(action.to_string()));
                object.insert("payload".into(), activity_payload.clone());
            }
            let started_at = crate::server::logging::now_ms();
            let result = self.network(proto::methods::gateway::ADMIN_ACTION, params).await;
            if let Some(identity) = access {
                crate::server::core::billing::keepalive::record_activity(account_id, started_at, &activity_payload, &result, identity);
            }
            result
        })
    }

    fn ensure_access_token<'a>(
        &'a self,
        _store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let value = self
                .network(proto::methods::gateway::TOKEN_ENSURE, self.with_account(account_id))
                .await?;
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| GatewayError::with_status(502, "插件返回的 token 不是字符串"))
        })
    }

    fn refresh_access_token<'a>(
        &'a self,
        _store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let value = self
                .network(proto::methods::gateway::TOKEN_REFRESH, self.with_account(account_id))
                .await?;
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| GatewayError::with_status(502, "插件返回的 token 不是字符串"))
        })
    }

    fn credentials_expiring(&self, _store: &AccountStore, _account_id: &str) -> bool {
        // 真正的实现在 [`Self::credentials_expiring_async`]。同步版返回
        // `false`（= 「不算临期，不用刷」）是**安全**的方向：后台维护跳过这家，
        // 而转发链路的懒刷新仍然生效。反过来返回 `true` 会让维护任务每轮
        // 白刷一次。
        false
    }

    fn refresh_models<'a>(
        &'a self,
        _store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async move {
            let mut params = self.base();
            params.insert("account".into(), Value::String(account_id.to_string()));
            params.insert("force".into(), Value::Bool(force));
            match self
                .network(proto::methods::gateway::MODELS_REFRESH, Value::Object(params))
                .await
            {
                Ok(value) => match serde_json::from_value::<proto::RefreshOutcome>(value) {
                    Ok(outcome) => {
                        if !outcome.refreshed {
                            return ModelRefreshOutcome {
                                refreshed: false,
                                count: outcome.count,
                                message: outcome.message,
                            };
                        }
                        let list = match self.control(
                            proto::methods::gateway::MODELS_LIST,
                            self.with_account(account_id),
                        ).await {
                            Ok(value) => match serde_json::from_value::<proto::ModelList>(value) {
                                Ok(list) => list,
                                Err(error) => return ModelRefreshOutcome::failed(
                                    format!("插件刷新后返回的模型清单无法解析: {error}"),
                                ),
                            },
                            Err(error) => return ModelRefreshOutcome::failed(error.message),
                        };
                        let count = super::rpc_cache::set_remote(self.id, list);
                        if count == 0 {
                            return ModelRefreshOutcome::failed("插件刷新后没有可用模型，已保留旧清单");
                        }
                        crate::server::core::providers::adapter::seed_hidden_visibility_defaults();
                        ModelRefreshOutcome {
                            refreshed: true,
                            count,
                            message: outcome.message,
                        }
                    }
                    Err(e) => ModelRefreshOutcome::failed(format!("插件返回形状不对: {e}")),
                },
                Err(e) => ModelRefreshOutcome::failed(e.message),
            }
        })
    }

    fn query_usage<'a>(
        &'a self,
        _store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            self.network(proto::methods::gateway::USAGE_QUERY, self.with_account(account_id))
                .await
        })
    }

    fn forward_conversation<'a>(
        &'a self,
        _store: &'a AccountStore,
        account_id: &'a str,
        body: &'a Value,
        client_headers: &'a HeaderMap,
        proxy: Option<ResolvedProxy>,
        stream: bool,
        telemetry: &'a Arc<RequestTelemetry>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ForwardOutcome, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut params = self.base();
            params.insert("account".into(), Value::String(account_id.to_string()));
            params.insert("proxy".into(), proxy.map_or(Value::Null, |proxy| wire_proxy(proxy)));
            params.insert("body".into(), body.clone());
            params.insert("headers".into(), headers_json(client_headers));
            // ── 恒发 `stream: true` ────────────────────────────────
            // 客户端要不要流式是**网关**的事：非流式时网关自己把帧聚合成
            // 一个完整 JSON（`aggregate_frame_stream`，与内置家同一份代码）。
            // 这样插件作者只写一条路径（推 chat SSE 帧），不必再实现一份
            // 「聚合成本」—— 两份聚合规则迟早会分叉，而分叉的症状是
            // 「流式与非流式的正文不一样」，极难排查。
            params.insert("stream".into(), Value::Bool(true));
            params.insert("session".into(), Value::String(session_of(client_headers)));
            // 发送名与思考档位同 `build_chat_request_async`：body 里已经是
            // 网关改好的名字（模型改写发生在这之前）。
            let wire_model = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
            params.insert("wire_model".into(), Value::String(wire_model.clone()));

            let call = self
                .host
                .open_stream(proto::methods::gateway::CONVERSATION, Value::Object(params))
                .await
                .map_err(|e| GatewayError::with_status(503, format!("提供商插件不可用：{e}")))?;
            let id = call.id;
            let mut events = call.events;

            // ── 等 head：这段时间里插件在做真正的上游请求 ────────────────
            // 超时用「等响应头」那份预算（设置页「请求超时」第二项）：
            // 有状态家在这一步要跑完 round + turn 的首帧，比无状态家更久，
            // 但它仍是「客户端还没收到任何内容」的阶段。
            //
            // ── 为什么这里等的是「第一条事件」而不是「head 帧」──────────
            // 插件可能在发 head **之前**就失败（凭证缺失、上游连不上），
            // 那时它给的是最终应答或 chunk（错误体）。只盯 head 会让网关
            // 白等满整个超时，然后报一句「超时」—— 而真正的原因就在通道里。
            let budget = Duration::from_millis(crate::server::config::timeout_settings().headers_ms());
            let first = match tokio::time::timeout(budget, events.recv()).await {
                Ok(Some(event)) => event,
                // 通道关了：宿主清表（进程没了）
                Ok(None) => {
                    self.host.close_stream(id);
                    return Err(GatewayError::with_status(
                        502,
                        "提供商插件在给出响应头之前退出",
                    ));
                }
                Err(_) => {
                    // 超时：取消掉这条调用，否则插件会一直跑到上游自己超时，
                    // 而客户端早就拿到错误了 —— 那份上游连接是白占的。
                    self.host.abort(id).await;
                    self.host.close_stream(id);
                    return Err(GatewayError::with_status(
                        502,
                        format!("等待上游响应头超时（{}秒）", budget.as_secs()),
                    ));
                }
            };

            let head = match first {
                super::StreamEvent::Head(head) => head,
                // 插件自己报了错：原样转给客户端（文案是它写的，比网关猜的准）
                super::StreamEvent::Done(Err(error)) => {
                    self.host.close_stream(id);
                    return Err(GatewayError::with_status(502, error.message));
                }
                super::StreamEvent::Done(Ok(_)) => {
                    self.host.close_stream(id);
                    return Err(GatewayError::with_status(
                        502,
                        "插件在没有给出响应头的情况下结束了会话转发",
                    ));
                }
                super::StreamEvent::Failed(message) => {
                    self.host.close_stream(id);
                    return Err(GatewayError::with_status(502, message));
                }
                // 先到的居然是正文：协议要求先 head，但把它当「状态码 200」
                // 继续处理比报错有用 —— 少一帧头不至于让整条流作废。
                super::StreamEvent::Chunk(bytes) => {
                    let status = 200;
                    return self
                        .finish_stream(id, status, stream, events, Some(bytes), wire_model, telemetry)
                        .await;
                }
            };

            self.finish_stream(id, head.status, stream, events, None, wire_model, telemetry)
                .await
        })
    }

    fn retry_advice(&self, _error_body: &Value, _attempt: usize, _budget: usize) -> Option<RetryAdvice> {
        // 真正的实现在 [`Self::retry_advice_async`]（同步签名没法 await）。
        // 同步版返回 None = 不退避：编排层还有 `transient_retry_advice` /
        // `fallback_retry_advice` 两层通用兜底，不会因此丢掉重试能力。
        None
    }

    fn retry_advice_async<'a>(
        &'a self,
        error_body: &'a Value,
        attempt: usize,
        budget: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<RetryAdvice>> + Send + 'a>> {
        Box::pin(async move {
            let mut params = self.base();
            params.insert("body".into(), error_body.clone());
            params.insert("attempt".into(), Value::from(attempt));
            params.insert("budget".into(), Value::from(budget));
            let value = match self
                .control(proto::methods::gateway::RETRY_ADVICE, Value::Object(params))
                .await
            {
                Ok(value) => value,
                // 问不到 = 不重试。与「这家不声明」同一处置，安全方向。
                Err(_) => return None,
            };
            if value.is_null() {
                return None;
            }
            serde_json::from_value::<proto::RetryAdvice>(value)
                .ok()
                .map(|advice| RetryAdvice {
                    delay_ms: advice.delay_ms,
                    reason: advice.reason,
                })
        })
    }

    fn retry_decision_async<'a>(
        &'a self,
        status: u16,
        error_body: &'a Value,
        attempt: usize,
        budget: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RetryDecision> + Send + 'a>> {
        Box::pin(async move {
            let mut params = self.base();
            params.insert("status".into(), Value::from(status));
            params.insert("body".into(), error_body.clone());
            params.insert("attempt".into(), Value::from(attempt));
            params.insert("budget".into(), Value::from(budget));
            let method = crate::server::core::providers::retry::RETRY_DECISION_METHOD;
            match self.control(method, Value::Object(params)).await {
                Ok(value) => serde_json::from_value(value).unwrap_or_else(|_| RetryDecision::Stop {
                    reason: "提供商插件返回的重试决定无法解析，停止本次请求".to_string(),
                }),
                Err(error) if error.status_code == 501 => {
                    match self.retry_advice_async(error_body, attempt, budget).await {
                        Some(advice) => RetryDecision::Retry {
                            delay_ms: advice.delay_ms,
                            reason: advice.reason,
                        },
                        None => RetryDecision::Default,
                    }
                }
                Err(error) => RetryDecision::Stop {
                    reason: format!("提供商插件无法给出重试决定，停止本次请求：{}", error.message),
                },
            }
        })
    }

    fn reasoning_patch(&self, _level: &str, _model: &str, _body: &Value) -> ReasoningPatch {
        // 真正的实现在 [`Self::reasoning_patch_async`]。同步版 Skip = 不注入
        // （body 一个字节不动），是安全兜底。
        ReasoningPatch::Skip { reason: "插件 provider 的档位翻译要走异步入口" }
    }

    fn build_login_url(&self) -> Option<(String, String)> {
        // 同步版无法走 RPC（要 await）。真正的实现在
        // [`Self::build_login_url_async`]，`core::login` 已改为调那个。
        None
    }

    fn build_login_url_async<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Option<(String, String)>> + Send + 'a>,
    > {
        Box::pin(async move {
            let start = self.login_start_async().await?;
            // `flow` 就是回调侧要比对的 state（插件自己生成、自己保管）
            Some((start.url, start.flow))
        })
    }

    /// 完整的登录启动信息：**保留 `poll_interval_ms`** —— 它是「等回调」与
    /// 「自己轮询」两种流程的唯一判据（见 `ProviderAdapter::login_start_async`
    /// 的说明）。
    fn login_start_async<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Option<agent2api_provider_protocol::LoginStart>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let params = serde_json::json!({
                "provider": self.id,
                // `method` 是「用哪种登录方式」，与流程形态无关：
                // 回调型与轮询型都由插件在 `LoginStart.poll_interval_ms` 里回答。
                "method": "browser",
                "payload": {},
            });
            let value = self
                .control(proto::methods::gateway::LOGIN_START, params)
                .await
                .ok()?;
            let start: proto::LoginStart = serde_json::from_value(value).ok()?;
            if start.url.is_empty() || start.flow.is_empty() {
                return None;
            }
            Some(start)
        })
    }

    /// 轮询一次登录进度（轮询型流程：授权回调在上游域名终结，永远不回到本机）。
    fn login_poll_async<'a>(
        &'a self,
        flow: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<proto::LoginPoll, GatewayError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            // 轮询本身要出网（问上游「授权完了没」），用网络超时
            let params = serde_json::json!({ "provider": self.id, "flow": flow });
            let value = self
                .network(proto::methods::gateway::LOGIN_POLL, params)
                .await?;
            serde_json::from_value::<proto::LoginPoll>(value).map_err(|e| {
                GatewayError::with_status(502, format!("插件返回的登录进度无法解析：{e}"))
            })
        })
    }

    fn normalize_account<'a>(
        &'a self,
        _store: &'a AccountStore,
        input: proto::NormalizeInput,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<proto::NormalizedAccount, GatewayError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            // 探活可能要出网（取昵称 / 验令牌），所以用宽超时
            let params = serde_json::to_value(&input).map_err(|e| {
                GatewayError::with_status(400, format!("添加账号的参数无法序列化：{e}"))
            })?;
            let value = self
                .network(proto::methods::gateway::ACCOUNT_NORMALIZE, params)
                .await?;
            serde_json::from_value::<proto::NormalizedAccount>(value).map_err(|e| {
                GatewayError::with_status(502, format!("插件返回的账号形状不对：{e}"))
            })
        })
    }

    /// 轮询型登录拿到账号后落库：与回调型共用 [`Self::save_login_account`]。
    ///
    /// ── 为什么必须共用 ──────────────────────────────────────────
    /// 两条路（回调换码 / 轮询取账号）都以 `NormalizedAccount` 收尾，落库规则
    /// （id 取 identity、名字取 `public.name`、插件账号走 `plugin_upsert_account`）
    /// 只能有一份 —— 分叉的症状是「网页登录的账号与手动添加的账号在列表里
    /// 长得不一样」。
    fn persist_login_account(
        &self,
        account: &proto::NormalizedAccount,
    ) -> Result<String, GatewayError> {
        self.save_login_account(account)
    }

    fn exchange_login_code<'a>(
        &'a self,
        _store: &'a AccountStore,
        code: &'a str,
        state: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let params = serde_json::json!({
                "provider": self.id,
                "flow": state,
                // `code` 是回调里的一次性凭据；`url` 给整条回调 URL ——
                // 有些家（OrcaTerm）要从 URL 的查询串里取参数，只给 code
                // 会让它拿不到。
                "code": code,
                "url": code,
            });
            let value = self
                .network(proto::methods::gateway::LOGIN_CALLBACK, params)
                .await?;
            let poll: proto::LoginPoll = serde_json::from_value(value)
                .map_err(|e| GatewayError::with_status(502, format!("插件返回的登录结果无法解析：{e}")))?;
            match poll {
                proto::LoginPoll::Done { account } => self.save_login_account(&account),
                proto::LoginPoll::Failed { message } => {
                    Err(GatewayError::with_status(400, message))
                }
                // 回调型流程拿到 Pending 说明插件还没处理完 —— 对这条链路
                // 是协议错误（网关不会再来第二次）
                proto::LoginPoll::Pending { .. } => Err(GatewayError::with_status(
                    502,
                    "插件没有完成登录回调（返回了 pending）",
                )),
            }
        })
    }

    /// 插件签到：`checkin.claim` RPC → 翻成 `{success, msg}`。
    ///
    /// ── 为什么在这里翻译而不是让插件直接给那个形状 ────────────────
    /// 协议里的 `CheckinReport` 有六档 outcome（`claimed` / `done` /
    /// `ineligible` / `inactive` / `captcha` / `failed`），比内置家那个
    /// 两字段形状**多一层语义**（「今天已签过」与「刚签到」必须分开：前者
    /// 不写 `checkinAt` 以外的状态，后者要播报积分）。网关侧把它们折成
    /// `{success, msg}` 时仍保留 outcome，Done 用 alreadyCompleted 表达，
    /// 完成判定与签到记录据此区分本次领取、已领取和无活动。
    fn checkin_claim<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        self.checkin_claim_with_options(store, account_id, false)
    }

    fn plugin_account_view<'a>(
        &'a self,
        secret: &'a Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut params = self.base();
            params.insert("secret".into(), secret.clone());
            self.control(proto::methods::gateway::ACCOUNT_VIEW, Value::Object(params)).await
        })
    }

    fn checkin_claim_with_options<'a>(
        &'a self,
        _store: &'a AccountStore,
        account_id: &'a str,
        manual: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut params = self.base();
            params.insert("account".into(), Value::String(account_id.to_string()));
            params.insert("manual".into(), Value::Bool(manual));
            let value = self.network(proto::methods::gateway::CHECKIN_CLAIM, Value::Object(params)).await?;
            let retry_at = value.get("retryAt").cloned();
            let report: proto::CheckinReport = serde_json::from_value(value).map_err(|e| {
                GatewayError::with_status(502, format!("插件返回的签到结果无法解析：{e}"))
            })?;
            let (success, default_message) = match report.outcome {
                proto::CheckinOutcome::Claimed => (true, "签到成功"),
                proto::CheckinOutcome::Done => (false, "今天已经签过了"),
                proto::CheckinOutcome::Ineligible => (false, "该账号不参与签到"),
                proto::CheckinOutcome::Inactive => (false, "当前没有可领的活动"),
                proto::CheckinOutcome::Captcha => {
                    (false, "上游要求验证码，请到官方客户端手动签到")
                }
                proto::CheckinOutcome::Failed => (false, "签到失败"),
            };
            let message = report
                .message
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .unwrap_or(default_message);
            let mut claim = serde_json::json!({
                "success": success,
                "alreadyCompleted": report.outcome == proto::CheckinOutcome::Done,
                "msg": message,
                "outcome": format!("{:?}", report.outcome).to_lowercase(),
                "credit": report.credit,
                "streak": report.streak,
            });
            if let Some(retry_at) = retry_at {
                claim["retryAt"] = retry_at;
            }
            Ok(claim)
        })
    }

    /// 插件领取：`claim` RPC，结果原样透给调用方。
    fn claim<'a>(
        &'a self,
        _store: &'a AccountStore,
        account_id: &'a str,
        claim: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut params = self.base();
            params.insert("account".into(), Value::String(account_id.to_string()));
            params.insert("claim".into(), Value::String(claim.to_string()));
            self.network(proto::methods::gateway::CLAIM, Value::Object(params))
                .await
        })
    }

    /// 插件领取状态：`claim.status` RPC（只读，结果原样透出）。
    fn claim_status<'a>(
        &'a self,
        _store: &'a AccountStore,
        account_id: &'a str,
        refresh: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut params = self.with_account(account_id);
            if refresh {
                if let Some(object) = params.as_object_mut() {
                    object.insert("refresh".into(), Value::Bool(true));
                }
            }
            self.network(proto::methods::gateway::CLAIM_STATUS, params)
                .await
        })
    }
}

/// 把插件的 `UpstreamKind` 映射到网关的 `UpstreamResponse`。
///
/// ── 为什么这个映射必须穷举 ──────────────────────────────────
/// 漏一个变体在 `panic=abort` 下是编译期错误（穷举 match 会拦住），
/// 所以这里刻意不写 `_ =>` 兜底 —— 将来加协议时编译器会提醒。
pub(crate) fn upstream_response(kind: proto::UpstreamKind) -> UpstreamResponse {
    match kind {
        proto::UpstreamKind::Chat => UpstreamResponse::Chat,
        proto::UpstreamKind::Anthropic => UpstreamResponse::Anthropic,
        proto::UpstreamKind::Ndjson => UpstreamResponse::AiSdkNdjson,
        proto::UpstreamKind::Gemini => UpstreamResponse::AntigravityGemini,
        proto::UpstreamKind::ChatglmSnapshot => UpstreamResponse::ChatglmSnapshot,
    }
}

/// 把插件的 `ErrorClass` 映射到网关的 `UpstreamErrorClass`。
///
/// ── 两边的形状刻意一致 ──────────────────────────────────────
/// 协议里的四档与网关的四档是**同一套语义**（那是编排层唯一认识的四档，
/// 见 `ProviderAdapter::classify_error` 的说明）。这里只做字段搬运。
fn upstream_error_class(class: proto::ErrorClass) -> UpstreamErrorClass {
    match class {
        proto::ErrorClass::QuotaLimited { reset_at, message, upstream_code, status } => {
            UpstreamErrorClass::QuotaLimited { reset_at, message, upstream_code, status }
        }
        proto::ErrorClass::TokenExpired { message } => {
            UpstreamErrorClass::TokenExpired { message }
        }
        proto::ErrorClass::ContentBlocked { status, message, upstream_code } => {
            UpstreamErrorClass::ContentBlocked { status, message, upstream_code }
        }
        proto::ErrorClass::Fatal { status, message, upstream_code } => {
            UpstreamErrorClass::Fatal { status, message, upstream_code }
        }
    }
}

impl RpcAdapter {
    /// 把登录回调产出的账号落库。
    ///
    /// ── 为什么 id 由插件给 ──────────────────────────────────────
    /// 插件在登录完成时已经算出了稳定身份（uid / 邮箱 / 设备码）。用那个做
    /// id 才能让「同一账号登录两次」被识别成更新而不是新增 —— 而网关不知
    /// 道任何一家的身份规则。
    fn save_login_account(&self, account: &proto::NormalizedAccount) -> Result<String, GatewayError> {
        let Some(store) = crate::server::core::account_store::plugin_accounts::shared() else {
            return Err(GatewayError::with_status(
                503,
                "账号库尚未就绪，无法保存登录结果",
            ));
        };
        let id = account.identity.clone();
        let name = account
            .public
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(self.label);
        let record = store
            .plugin_upsert_account(&id, self.id, name, &account.secret, &account.public, account.desktop)
            .map_err(|message| GatewayError::with_status(500, message))?;
        Ok(record.get("id").and_then(Value::as_str).unwrap_or(&id).to_string())
    }

    /// head 已到之后的三条分支：错误状态 / 非流式聚合 / 流式交出去。
    ///
    /// ── 为什么单独一个函数 ──────────────────────────────────────
    /// 这三条分支都要先拿到 head，而 head 可能来自「正常的第一条事件」或
    /// 「先到的正文」（见 `forward_conversation` 的说明）。把分支放在一处，
    /// 两条入口就不会各写一份 —— 那种分叉的症状是「某一种到达顺序下
    /// 错误处理被跳过」。
    async fn finish_stream(
        &self,
        id: i64,
        status: u16,
        stream: bool,
        events: tokio::sync::mpsc::UnboundedReceiver<super::StreamEvent>,
        pending: Option<Vec<u8>>,
        wire_model: String,
        telemetry: &Arc<RequestTelemetry>,
    ) -> Result<ForwardOutcome, GatewayError> {
        // ── 失败状态：先收齐正文再分类（与内置家的门控同一处置）────────
        // `status >= 400` 时插件仍会推 chunk（那是错误体的字节），但客户端
        // 要的是一份**完整的** JSON 错误，而不是一段 SSE。
        if status >= 400 {
            let mut events = events;
            let mut collected = pending.unwrap_or_default();
            collected.extend(collect(&mut events).await);
            let text = String::from_utf8_lossy(&collected);
            let error = match serde_json::from_str::<Value>(&text) {
                Ok(value) => value,
                // 不是 JSON：把原文塞进 message，分类器仍能按文案判（各家的
                // 错误页形态不同，逐家解析没有意义）。
                Err(_) => Value::String(text.to_string()),
            };
            // 分类走同一个 RPC 入口（插件知道自己的业务码含义）。
            let class = self.classify_error_async(status, &error).await;
            self.host.close_stream(id);
            return Err(conversation_error(status, class, &error));
        }

        let source = super::RpcStream::with_pending(
            self.host.clone(),
            id,
            events,
            pending,
            telemetry.clone(),
        );

        // ── 非流式：网关把帧聚合掉（插件恒按 stream:true 推）───────────
        // 客户端要不要流式是**网关**的事：非流式时网关自己聚合（与内置家
        // 同一份 `aggregate_frame_stream`）。插件作者因此只写一条路径，
        // 不必再实现一份「聚合成本」—— 两份规则迟早分叉，而症状是
        // 「流式与非流式的正文不一样」，极难排查。
        if !stream {
            let policy = crate::server::core::upstream::sse::FramePolicy::assembled(
                self.capabilities
                    .sse_model_rewrite
                    .then(|| crate::server::core::upstream::sse::ModelRewrite {
                        requested: wire_model,
                    }),
                self.capabilities.sse_strip_newline_chunks,
                crate::server::config::strip_newline_keepalive(),
            );
            let aggregated = crate::server::core::upstream::aggregate::aggregate_frame_stream(
                Box::pin(source),
                telemetry.clone(),
                policy,
            )
            .await;
            // 流被 drop 时会 `close_stream` + abort（见 `RpcStream`）
            return match aggregated {
                Ok(done) => Ok(ForwardOutcome::Completion { body: done.body }),
                Err(error) => Err(error),
            };
        }

        Ok(ForwardOutcome::Stream { status, stream: Box::new(source) })
    }
}

/// 把一批运行期字符串泄漏成 `&'static [&'static str]`。
///
/// ── 为什么需要它 ────────────────────────────────────────────
/// trait 的 `stream_refusal_finishes` / `stream_refusal_markers` 返回
/// `&'static [&'static str]`（内置家返回字面量）。插件给的是运行期数据。
/// 每次握手泄漏几十字节、一个适配器只做一次 —— 这是「不改 21 家签名」与
/// 「跨进程拿运行期数据」之间最小的代价。
fn leak_slice(values: Vec<String>) -> &'static [&'static str] {
    let leaked: Vec<&'static str> = values.into_iter().map(leak_str).collect();
    Box::leak(leaked.into_boxed_slice())
}

/// 单个字符串的泄漏版（见 [`leak_all`]）。
fn leak_str(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

/// 把网关解析好的出口转成线上形态。
///
/// ── 为什么不复用 `proxy_of` ──────────────────────────────────
/// 那个函数从**账号会话形态**里取（JSON），而 `forward_conversation` 拿到
/// 的是已经解析好的结构体（编排层在选路时就解好了）。多绕一趟 JSON 只会
/// 多一次可能失败的转换 —— 失败时静默降级成直连，而用户明明配了代理。
fn wire_proxy(proxy: ResolvedProxy) -> Value {
    serde_json::to_value(proto::WireProxy {
        protocol: proxy.protocol,
        host: proxy.host,
        port: proxy.port,
        username: proxy.username,
        password: proxy.password,
    })
    .unwrap_or(Value::Null)
}

/// 从入站头取客户端会话标识（`x-session-id`）。
///
/// 归一化规则与内置家逐字一致（见 `catpaw::conversation::session_id_from_headers`）：
/// 空 / 超 256 字符一律当「没有会话 id」—— 超长 id 截断会让两个不同的
/// 客户端会话在前 256 位相同时碰撞，那比「没有会话」更坏。
fn session_of(headers: &HeaderMap) -> String {
    let value = headers
        .get("x-session-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .trim();
    if value.is_empty() || value.chars().count() > 256 {
        return String::new();
    }
    value.to_string()
}

/// 收齐一条流式调用的全部 chunk（失败状态码的分支用）。
///
/// 上限与帧上限同源：错误体不该有几百 KB，真到了那个量级说明上游返回的
/// 不是错误 JSON 而是别的什么东西（一个 HTML 页面、一段日志）。截断比
/// 无限累积安全 —— 分类器只需要开头那段文案。
async fn collect(events: &mut tokio::sync::mpsc::UnboundedReceiver<super::StreamEvent>) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    while let Some(event) = events.recv().await {
        match event {
            super::StreamEvent::Chunk(bytes) => {
                if out.len() + bytes.len() > proto::MAX_LINE_BYTES {
                    break;
                }
                out.extend_from_slice(&bytes);
            }
            // 错误体的字节已经收完了：最终应答只是「我说完了」，不必再等
            super::StreamEvent::Done(_) | super::StreamEvent::Failed(_) => break,
            // 失败状态下再来一个 head 没有意义（状态码已经定了），跳过
            super::StreamEvent::Head(_) => {}
        }
    }
    out
}

/// 把「上游失败状态 + 分类结果」组装成网关错误。
///
/// ── 与编排层同一组装口径 ────────────────────────────────────
/// 见 `upstream::provider_loop` 的 `OutboundFailure` 构造：状态码取分类里
/// 那个（分类器可能把它改小，例如 402 → 429），文案原样用，上游码带上。
fn conversation_error(status: u16, class: UpstreamErrorClass, body: &Value) -> GatewayError {
    match class {
        UpstreamErrorClass::QuotaLimited { status, message, upstream_code, .. }
        | UpstreamErrorClass::ContentBlocked { status, message, upstream_code }
        | UpstreamErrorClass::Fatal { status, message, upstream_code } => {
            let text = if message.is_empty() { describe_body(body) } else { message };
            GatewayError::with_status(status as i32, text).with_optional_code(upstream_code)
        }
        UpstreamErrorClass::TokenExpired { message } => {
            let text = if message.is_empty() { describe_body(body) } else { message };
            GatewayError::with_status(status as i32, text)
        }
    }
}

/// 错误体没有可读文案时的兜底（与内置家的 `上游返回 {status}: …` 同形态）。
fn describe_body(body: &Value) -> String {
    match body {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// 从转发会话里取账号库主键；空串表示默认登录态。
///
/// 上游 `account.uid` 不一定等于账号主键，不能用它索引账号库。
fn account_id_of(account: &Value) -> String {
    account
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// 从账号会话形态里取代理（转成线上形态）。
fn proxy_of(account: &Value) -> Value {
    let Some(proxy) = account.get("proxy") else {
        return Value::Null;
    };
    if proxy.is_null() {
        return Value::Null;
    }
    match serde_json::from_value::<proto::WireProxy>(proxy.clone()) {
        Ok(wire) => serde_json::to_value(wire).unwrap_or(Value::Null),
        // 代理数据坏了：按「跟随默认出口」处理（与网关 `ResolvedProxy::from_json`
        // 的第三态口径一致 —— 可用性优先，出口测试才如实报错）。
        Err(_) => Value::Null,
    }
}

/// 把客户端入站头转成协议里的 `BTreeMap<String, String>`。
///
/// ── 为什么要小写化 ──────────────────────────────────────────
/// 协议里约定头名小写（与 HTTP/2 一致）。插件按小写键查，网关按小写键给 ——
/// 两边各写一套大小写规则的下场是「插件收不到某个头」而没有任何报错。
fn headers_json(headers: &HeaderMap) -> Value {
    let mut map = serde_json::Map::new();
    for (name, value) in headers {
        if let Ok(text) = value.to_str() {
            map.insert(name.as_str().to_ascii_lowercase(), Value::String(text.to_string()));
        }
    }
    Value::Object(map)
}
