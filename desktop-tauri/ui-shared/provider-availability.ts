// 稳定 ID 仍用于存量账号；Rust 执行体恢复前不开放添加和调用。
export const SUSPENDED_PROVIDER_IDS: ReadonlySet<string> = new Set([
  'cursor',
  'kiro',
  'devin',
  'factory',
  'zed',
  'grok',
  'minimax-code',
  'minimax-code-global',
  'qwenwork',
  'qwenwork-enterprise',
  'kimi-work',
  'opendesign',
  'nous-portal',
  'claude-subscription',
  'codex-subscription',
  'chatgpt-subscription',
  'copilot-subscription',
  'gemini-subscription',
])

export function isSuspendedProvider(id: string): boolean {
  return SUSPENDED_PROVIDER_IDS.has(id)
}
