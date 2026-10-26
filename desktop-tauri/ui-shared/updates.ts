export type ForceUpdatePolicy = { required: boolean; minimumVersion: string; targetVersion: string; message: string; source?: string }

export type UpdateChannels = {
  stable: boolean
  beta: boolean
  saved?: boolean
}

export type UpdateCheckPlan = {
  enabled: boolean
  interval: number
  saved?: boolean
}

export type UpdateSettingsMode = 'follow' | 'custom'

type Translator = (key: string, params?: Record<string, string | number>) => string

export function githubErrorMessage(error: unknown, t: Translator, settings: 'update' | 'plugin'): string {
  const message = error instanceof Error ? error.message : String(error ?? '')
  if (message === 'GitHub 接口限流中，请在「更新设置」的「GitHub 令牌」中填写凭证，以提高请求限额。'
    || message === 'GitHub 接口限流中，请在「插件设置」的「GitHub 令牌」中填写凭证，以提高请求限额。') {
    return settings === 'plugin'
      ? t('GitHub 接口限流中，请在「插件设置」的「GitHub 令牌」中填写凭证，以提高请求限额。')
      : t('GitHub 接口限流中，请在「更新设置」的「GitHub 令牌」中填写凭证，以提高请求限额。')
  }
  const cooldown = /^GitHub 接口限流中，请在 (\d+) 秒后重试$/.exec(message)
  if (cooldown) return t('GitHub 接口限流中，请在 {seconds} 秒后重试', { seconds: cooldown[1] })
  if (message === 'GitHub 接口限流中，请稍后重试') return t('GitHub 接口限流中，请稍后重试')
  return message
}
