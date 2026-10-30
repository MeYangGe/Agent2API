import * as React from 'react'
import { Field, FieldLabel, FieldDescription, Input, Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@ui'
import { t } from '../i18n'
import type { AgentClient, AgentProfile } from './agent-types'
import type { CatalogModel } from './agent-catalog'
import { protocolLabel } from './agent-model'
import { getAgentStore } from './agent-state'

export function AgentSelect({ value, options, onChange, disabled, label }: {
  value: string; options: Array<{ value: string; label: string }>; onChange: (value: string) => void; disabled?: boolean; label: string
}) {
  const selected = options.find(option => option.value === value)?.label || value || t('请选择')
  return <Select value={value} disabled={disabled} onValueChange={next => { if (typeof next === 'string') onChange(next) }}>
    <SelectTrigger aria-label={label} title={selected} className='w-full'><SelectValue>{selected}</SelectValue></SelectTrigger>
    <SelectContent>{options.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
  </Select>
}

export function TextField({ label, fieldKey, value, onChange, hint, type = 'text', disabled, placeholder }: {
  label: string; fieldKey: string; value: string; onChange: (value: string) => void; hint?: string; type?: string; disabled?: boolean; placeholder?: string
}) {
  const id = React.useId()
  return <Field><FieldLabel htmlFor={id}>{label}<code className='ml-auto text-[10px] text-subtle'>{fieldKey}</code></FieldLabel>
    <Input id={id} type={type} autoComplete={type === 'password' ? 'new-password' : 'off'} spellCheck={false} value={value}
      onChange={event => onChange(event.currentTarget.value)} disabled={disabled} placeholder={placeholder} />
    {hint && <FieldDescription>{hint}</FieldDescription>}
  </Field>
}

export function ConnectionFields({ client, draft, update, disabled }: {
  client: AgentClient; draft: AgentProfile; update: (patch: Partial<AgentProfile>) => void; disabled: boolean
}) {
  const store = getAgentStore()
  const authOptions = client.id === 'zed' ? [{ value: 'preserve', label: t('在客户端管理认证') }]
    : [ ...(client.supportsGateway && draft.kind === 'gateway' ? [{ value: 'gateway', label: t('使用网关 Key') }] : []),
      { value: 'key', label: t('API Key') }, ...(client.supportsEnvKey ? [{ value: 'env', label: t('环境变量') }] : []),
      { value: 'none', label: t('无需认证') }, ...(draft.sourceId ? [{ value: 'preserve', label: t('保留导入的认证') }] : []) ]
  const switchSource = (kind: string) => {
    const baseUrl = kind === 'gateway' ? draft.protocol === 'anthropic' ? store.gateway : `${store.gateway}/v1` : draft.baseUrl
    update({ kind, baseUrl, authMode: client.id === 'zed' ? 'preserve' : kind === 'gateway' ? 'gateway' : draft.authMode === 'gateway' ? 'key' : draft.authMode })
  }
  return <div className='agent-connection-fields grid min-w-0 grid-cols-1 gap-4 sm:grid-cols-2'>
    <Field><FieldLabel>{t('配置来源')}</FieldLabel><AgentSelect label={t('配置来源')} value={draft.kind} disabled={disabled} onChange={switchSource}
      options={[...(client.supportsGateway ? [{ value: 'gateway', label: t('本网关') }] : []), { value: 'official', label: t('官方直连') }, { value: 'relay', label: t('第三方中转') }, { value: 'local', label: t('本机服务') }]} /></Field>
    <Field><FieldLabel>{t('客户端协议')}</FieldLabel><AgentSelect label={t('客户端协议')} value={draft.protocol} disabled={disabled || client.protocols.length === 1 && client.protocols.includes(draft.protocol)}
      onChange={protocol => update({ protocol, ...(draft.kind === 'gateway' ? { baseUrl: protocol === 'anthropic' ? store.gateway : `${store.gateway}/v1` } : {}) })}
      options={client.protocols.map(value => ({ value, label: protocolLabel(value) }))} />
      <FieldDescription>{t('这里只选择客户端实际支持的协议，不改变上游提供商协议。')}</FieldDescription></Field>
    <div className='sm:col-span-2'><TextField label={t('接口基址')} fieldKey={urlField(client.id)} value={draft.baseUrl} onChange={baseUrl => update({ baseUrl })} disabled={disabled}
      hint={client.id === 'workbuddy' ? t('保存时自动写入完整的 /chat/completions 路径。') : draft.protocol === 'gemini' ? t('填写服务根地址，不要附加 /v1beta 或 generateContent 路径。') : draft.protocol === 'anthropic' ? t('不要添加 /v1/messages；客户端会自行追加请求路径。') : t('通常以 /v1 结尾，不要附加具体请求路径。')} /></div>
    <Field><FieldLabel>{t('认证方式')}</FieldLabel><AgentSelect label={t('认证方式')} value={draft.authMode} options={authOptions} onChange={authMode => update({ authMode })} disabled={disabled} /></Field>
    {draft.authMode === 'gateway' && store.requiresKey && <Field><FieldLabel>{t('网关 Key')}</FieldLabel>
      <AgentSelect label={t('网关 Key')} value={draft.gatewayKeyId} disabled={disabled} onChange={gatewayKeyId => update({ gatewayKeyId })}
        options={[{ value: '', label: t('第一把启用的 Key') }, ...store.gatewayKeys.map(key => ({ value: key.id, label: key.name }))]} />
      <FieldDescription>{t('应用时使用当前启用的真实 Key；预览会隐藏密钥。')}</FieldDescription></Field>}
    {draft.authMode === 'gateway' && !store.requiresKey && <p className='text-xs text-subtle'>{t('当前网关未启用 API Key 鉴权；占位值只用于客户端要求非空密钥的场景。')}</p>}
    {draft.authMode === 'key' && <TextField label={t('API Key')} fieldKey={client.id === 'codex' ? 'experimental_bearer_token' : client.id === 'claude-code' ? 'env.ANTHROPIC_API_KEY' : 'apiKey'} type='password' value={draft.apiKey} onChange={apiKey => update({ apiKey })} disabled={disabled}
      placeholder={draft.hasApiKey ? t('留空保留已保存密钥') : t('请输入密钥')} hint={t('方案中的密钥加密保存；客户端原生配置可能要求明文。')} />}
    {draft.authMode === 'env' && <TextField label={t('环境变量名')} fieldKey='env_key' value={draft.envKey} onChange={envKey => update({ envKey })} disabled={disabled}
      hint={t('只保存变量引用，不修改系统环境；请在启动客户端的环境里设置变量值。')} />}
    {client.id === 'zed' && <p className='text-xs leading-relaxed text-subtle'>{t('请在 Zed 提供商设置中保存 API Key；本页不修改系统凭证存储。')}</p>}
  </div>
}

function urlField(id: string): string {
  return { 'claude-code': 'env.ANTHROPIC_BASE_URL', codex: 'model_providers.<id>.base_url', 'gemini-cli': 'GOOGLE_GEMINI_BASE_URL', opencode: 'provider.<id>.options.baseURL', zcode: 'config.api.baseUrl', workbuddy: 'models[].url', cline: 'providers.<id>.settings.baseUrl', 'kimi-code': 'providers.<id>.base_url', crush: 'providers.<id>.base_url', zed: 'language_models.openai_compatible.<id>.api_url' }[id] || 'baseUrl'
}

export function SingleModelFields({ client, draft, update, catalog, disabled }: {
  client: AgentClient; draft: AgentProfile; update: (patch: Partial<AgentProfile>) => void; catalog: CatalogModel[]; disabled: boolean
}) {
  const selected = catalog.find(model => model.id === draft.model)
  const allowed = client.id === 'claude-code' ? ['low','medium','high','xhigh','max','auto'] : ['none','minimal','low','medium','high','xhigh']
  const levels = selected?.reasoningLevels.filter(level => allowed.includes(level)) || []
  return <>
    {draft.kind === 'gateway' && catalog.length > 0 && <Field><FieldLabel>{t('从网关目录选择模型')}</FieldLabel>
      <AgentSelect label={t('从网关目录选择模型')} value={draft.model} options={catalog.map(model => ({ value: model.id, label: model.label }))} disabled={disabled}
        onChange={model => update({ model, effort: '' })} /></Field>}
    <TextField label={t('模型 ID')} fieldKey={client.id === 'claude-code' ? 'model / ANTHROPIC_MODEL' : client.id === 'gemini-cli' ? 'model.name' : 'model'} value={draft.model} onChange={model => update({ model, effort: '' })} disabled={disabled}
      hint={t('使用真实模型 ID，不要填写模型展示名。')} />
    {client.id === 'claude-code' && <>
      <TextField label={t('Opus 模型映射')} fieldKey='ANTHROPIC_DEFAULT_OPUS_MODEL' value={draft.claudeModels.opus} onChange={opus => update({ claudeModels: { ...draft.claudeModels, opus } })} disabled={disabled} />
      <TextField label={t('Sonnet 模型映射')} fieldKey='ANTHROPIC_DEFAULT_SONNET_MODEL' value={draft.claudeModels.sonnet} onChange={sonnet => update({ claudeModels: { ...draft.claudeModels, sonnet } })} disabled={disabled} />
      <TextField label={t('Haiku 模型映射')} fieldKey='ANTHROPIC_DEFAULT_HAIKU_MODEL' value={draft.claudeModels.haiku} onChange={haiku => update({ claudeModels: { ...draft.claudeModels, haiku } })} disabled={disabled} />
      <TextField label={t('子 Agent 模型')} fieldKey='CLAUDE_CODE_SUBAGENT_MODEL' value={draft.claudeModels.subagent} onChange={subagent => update({ claudeModels: { ...draft.claudeModels, subagent } })} disabled={disabled} />
    </>}
    {client.supportsEffort && (levels.length > 0 || draft.effort) && <Field><FieldLabel>{t('思考等级')}<code className='ml-auto text-[10px]'>{client.id === 'codex' ? 'model_reasoning_effort' : 'CLAUDE_CODE_EFFORT_LEVEL'}</code></FieldLabel>
      <AgentSelect label={t('思考等级')} value={draft.effort} disabled={disabled} onChange={effort => update({ effort })}
        options={[{ value: '', label: t('模型默认') }, ...[...new Set([...levels, ...(draft.effort ? [draft.effort] : [])])].map(value => ({ value, label: value }))]} /></Field>}
    {client.supportsEffort && levels.length === 0 && !draft.effort && <p className='text-xs text-subtle'>{t('模型未声明思考档位，保留客户端默认，不显示猜测选项。')}</p>}
  </>
}
