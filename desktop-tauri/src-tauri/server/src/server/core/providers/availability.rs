//! 停用路线的稳定身份；保留存量账号，不提供执行体或添加入口。

pub const BUILTIN_SUBSCRIPTION_IDS: [&str; 5] = [
    "claude-subscription",
    "codex-subscription",
    "chatgpt-subscription",
    "copilot-subscription",
    "gemini-subscription",
];

const PLUGIN_RUNTIME_IDS: [&str; 13] = [
    "cursor",
    "kiro",
    "devin",
    "factory",
    "zed",
    "grok",
    "minimax-code",
    "minimax-code-global",
    "qwenwork",
    "qwenwork-enterprise",
    "kimi-work",
    "opendesign",
    "nous-portal",
];

pub(crate) fn is_runtime_identity(id: &str) -> bool {
    BUILTIN_SUBSCRIPTION_IDS.contains(&id) || PLUGIN_RUNTIME_IDS.contains(&id)
}

pub fn is_suspended(id: &str) -> bool {
    is_runtime_identity(id)
}
