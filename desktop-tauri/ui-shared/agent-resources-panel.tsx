import { useCallback, useEffect, useState } from 'react'
import { Alert, Button, Card, Empty, Input, Label, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Textarea, confirmDialog } from '@ui'
import type { Translate } from './assistant-read-types'
import { resourceBase, type ResourceCapabilities, type ResourceKind, type ResourceRequest, type ResourcePreview, type PromptResource, type SkillsResource, type McpResource } from './agent-resources'

type Props = { clientId: string; kind: ResourceKind; capabilities?: ResourceCapabilities; request: ResourceRequest; t: Translate; notify: (text: string, bad?: boolean) => void }

export function AgentResourcesPanel({ clientId, kind, capabilities, request, t, notify }: Props) {
  const [data, setData] = useState<PromptResource | SkillsResource | McpResource | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [draft, setDraft] = useState('')
  const [selected, setSelected] = useState('')
  const [name, setName] = useState('')
  const [revision, setRevision] = useState('absent')
  const base = resourceBase(clientId)
  const supported = capabilities?.[kind] === true
  const load = useCallback(async () => {
    if (!supported) return
    setBusy(true); setError('')
    try {
      const result = await request<PromptResource | SkillsResource | McpResource>('GET', `${base}?kind=${kind}`)
      setData(result); setSelected(''); setName('')
      if (kind === 'prompt') { setDraft((result as PromptResource).content); setRevision((result as PromptResource).revision) }
      else { setDraft(''); setRevision(kind === 'mcp' ? (result as McpResource).revision : 'absent') }
    } catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { setBusy(false) }
  }, [base, kind, request, supported])
  useEffect(() => { setData(null); void load() }, [load])
  if (!supported) return <Alert className='mt-3'>{capabilities?.note || t('尚未支持这个客户端的资源管理')}</Alert>
  const prompt = kind === 'prompt' ? data as PromptResource | null : null
  const skills = kind === 'skills' ? data as SkillsResource | null : null
  const mcp = kind === 'mcp' ? data as McpResource | null : null
  const skill = skills?.items.find(item => item.id === selected)

  const pick = async (value: string) => {
    setSelected(value)
    if (kind === 'skills') {
      if (value === '__new__') { setName(''); setRevision('absent'); setDraft('---\nname: my-skill\ndescription: Describe when to use this skill.\n---\n\n'); return }
      setBusy(true)
      try {
        const detail = await request<PromptResource>('GET', `${base}?kind=skill&id=${encodeURIComponent(value)}`)
        setDraft(detail.content); setRevision(detail.revision)
      } catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)) }
      finally { setBusy(false) }
    } else {
      const server = mcp?.servers.find(item => item.name === value)
      setName(server?.name || '')
      const template = clientId === 'opencode' ? { type: 'local', command: [''] } : { command: '', args: [] }
      setDraft(JSON.stringify(server?.config || template, null, 2))
    }
  }

  const save = async (deleting = false) => {
    if (busy || !data) return
    setBusy(true); setError('')
    try {
      const payload: Record<string, unknown> = { kind, revision, delete: deleting }
      if (kind === 'prompt') payload.content = draft
      if (kind === 'skills') { payload.id = selected === '__new__' ? `0:${name.trim()}` : selected; payload.content = draft }
      if (kind === 'mcp') { payload.name = name.trim(); if (!deleting) payload.config = JSON.parse(draft) }
      const preview = await request<ResourcePreview>('POST', `${base}/preview`, payload)
      if (!preview.hasChanges) { notify(t('内容已经一致，无需写入')); return }
      const confirmed = await confirmDialog(<div className='flex flex-col gap-3'>
        <p>{t('所有操作作用在网关所在机器。')}</p>
        {preview.warnings.map((warning, index) => <p key={index}>{warning}</p>)}
        {preview.files.map(file => <div key={file.path}><code className='break-all'>{file.path}</code><pre className='max-h-72 overflow-auto whitespace-pre-wrap text-xs'>{file.diff.filter(line => line.kind !== 'same').map(line => `${line.kind === 'added' ? '+' : '−'} ${line.text}`).join('\n')}</pre></div>)}
      </div>, { title: deleting ? t('确认删除客户端资源') : t('确认写入客户端资源'), confirmText: t('确认') })
      if (!confirmed) return
      await request('POST', `${base}/apply`, { ...payload, token: preview.token })
      notify(t('客户端资源已保存并校验'))
      await load()
    } catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { setBusy(false) }
  }

  return <div className='mt-4 flex min-w-0 flex-col gap-3'>
    <div className='flex items-center justify-between gap-2'><p className='text-xs text-muted-foreground'>{capabilities.note}</p><Button size='sm' variant='outline' disabled={busy} onClick={() => void load()}>{t('刷新')}</Button></div>
    {error ? <Alert variant='destructive'>{error}</Alert> : null}
    {prompt ? <><code className='break-all text-xs'>{prompt.path}</code><Textarea rows={16} className='font-mono' value={draft} disabled={busy} onChange={event => setDraft(event.currentTarget.value)} /><Button disabled={busy} onClick={() => void save()}>{t('预览并保存')}</Button></> : null}
    {skills ? <>
      <code className='break-all text-xs'>{skills.roots.join('\n')}</code>
      <Select value={selected} disabled={busy} onValueChange={value => void pick(String(value))}>
        <SelectTrigger><SelectValue>{selected === '__new__' ? t('新建 Skill') : skill?.name || t('选择 Skill')}</SelectValue></SelectTrigger>
        <SelectContent><SelectItem value='__new__'>{t('新建 Skill')}</SelectItem>{skills.items.map(item => <SelectItem key={item.id} value={item.id}>{item.name}</SelectItem>)}</SelectContent>
      </Select>
      {!skills.items.length && !selected ? <Empty>{t('这个客户端还没有 Skill')}</Empty> : null}
      {selected ? <Card className='flex flex-col gap-3 p-3'>
        {selected === '__new__' ? <><Label>{t('Skill 目录名')}</Label><Input placeholder='my-skill' value={name} onChange={event => setName(event.currentTarget.value)} /></> : <code className='break-all text-xs'>{skill?.path}</code>}
        {skill?.description ? <p className='text-xs text-muted-foreground'>{skill.description}</p> : null}
        <Textarea rows={16} className='font-mono' value={draft} disabled={busy || skill?.linked} onChange={event => setDraft(event.currentTarget.value)} />
        <div className='flex gap-2'><Button disabled={busy || skill?.linked} onClick={() => void save()}>{t('预览并保存')}</Button>{selected !== '__new__' ? <Button variant='outline' disabled={busy || skill?.linked} onClick={() => void save(true)}>{t('删除 Skill')}</Button> : null}</div>
        <p className='text-xs text-muted-foreground'>{t('删除只移除 SKILL.md，辅助文件保留在原目录；不会执行其中的脚本。')}</p>
      </Card> : null}
    </> : null}
    {mcp ? <>
      <code className='break-all text-xs'>{mcp.path}</code>
      <Select value={selected} disabled={busy} onValueChange={value => void pick(String(value))}>
        <SelectTrigger><SelectValue>{selected === '__new__' ? t('新增 MCP') : selected || t('选择 MCP')}</SelectValue></SelectTrigger>
        <SelectContent><SelectItem value='__new__'>{t('新增 MCP')}</SelectItem>{mcp.servers.map(item => <SelectItem key={item.name} value={item.name}>{item.name}</SelectItem>)}</SelectContent>
      </Select>
      {selected ? <Card className='flex flex-col gap-3 p-3'>
        <Label>{t('MCP 服务名称')}</Label><Input value={name} disabled={selected !== '__new__' || busy} onChange={event => setName(event.currentTarget.value)} />
        <Label>{t('客户端原生 MCP 配置（JSON）')}</Label><Textarea rows={14} className='font-mono' value={draft} disabled={busy} onChange={event => setDraft(event.currentTarget.value)} />
        <div className='flex gap-2'><Button disabled={busy} onClick={() => void save()}>{t('预览并保存')}</Button>{selected !== '__new__' ? <Button variant='outline' disabled={busy} onClick={() => void save(true)}>{t('删除 MCP')}</Button> : null}</div>
        <p className='text-xs text-muted-foreground'>{t('仅修改所选 MCP 条目，保留其它配置和注释；保存不会启动服务。')}</p>
      </Card> : null}
    </> : null}
  </div>
}
