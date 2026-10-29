import * as React from 'react'
import { Badge, Button, Checkbox, Input, Switch , Label, Card, CardHeader } from '@ui'
import {
  patch, removeScheduleEntry, runScheduleEntry, saveKeepaliveModels,
  saveScheduleEntry, saveTravel, runTravelNow, store, toggleMaster,
} from './checkin-store'
import {
  formatClock, formatTime,
  type CheckinCenter, type CheckinResult, type ScheduleEntry,
} from './checkin-types'
import { t } from '../i18n'
import { confirmDialog } from '@ui'
import { Empty } from '@ui'
import { callJson } from './assistant-state'
import { ModelApiKeyField } from '../../../ui-shared/model-api-key-field'
import { useModelApiKeys } from '../../../ui-shared/use-model-api-keys'

/**
 * 「定时签到」标签页：总开关 + 条目列表 + 保活模型链 + 猫猫旅行。
 *
 * ── 编排在网关，页面只是视图 ────────────────────────────────
 * 一条定时条目可能横跨九家提供商；卸载插件不能让用户丢排程。所以排程、去重、
 * 错过补签全在网关（`core::auto_checkin` / `core::checkin_schedules`）。
 *
 * ── 两块「地区专属」面板 ────────────────────────────────────
 * 保活模型链（国际版）与猫猫旅行（国内版）由插件申报的 `adminPanels` 决定显不
 * 显示。它们**该跟着插件走** —— 那是插件独有的玩法知识，网关不该假装知道
 * 「国际版有日活保活这回事」。这与「签到中心菜单常驻」不矛盾：菜单是产品功能，
 * 这两个面板是某家提供商的特色。
 */

/* ─── 条目编辑表单 ─────────────────────────── */

function EditorForm({ entry, options, defaultRetryTime }: {
  entry: ScheduleEntry | null
  options: { id: string; label: string; builtinTime?: string | null }[]
  defaultRetryTime: string
}): React.ReactElement {
  const isNew = entry === null
  const [label, setLabel] = React.useState(entry?.label ?? '')
  // 新建默认 00:01：与后端 `DEFAULT_TIME` 同一个值，不在这里另立一个
  const [time, setTime] = React.useState(entry?.time || '00:01')
  const [providers, setProviders] = React.useState<string[]>(
    entry ? [...entry.providers] : options.map(item => item.id),
  )
  // 逐家时刻覆盖：只存用户真的改过的那些（空串 = 撤销覆盖）
  const [providerTimes, setProviderTimes] = React.useState<Record<string, string>>(
    entry ? { ...entry.providerTimes } : {},
  )
  const [retryTime, setRetryTime] = React.useState(entry?.retryTime ?? defaultRetryTime)
  const [retryGap, setRetryGap] = React.useState(String(entry?.retryGapMinutes ?? 10))
  const [retryLimit, setRetryLimit] = React.useState(String(entry?.retryLimit ?? 3))

  const canSave = time.trim() !== '' && providers.length > 0 && !store.saving

  const toggleProvider = (id: string, checked: boolean) => {
    setProviders(prev => (checked
      ? (prev.includes(id) ? prev : [...prev, id])
      : prev.filter(item => item !== id)))
  }

  /** 某家的生效时刻（用户覆盖 > 内置专属 > 条目时刻）—— 与后端同一套优先级 */
  const effectiveTime = (id: string): string => {
    const override = providerTimes[id]
    if (override) return override
    const builtin = options.find(item => item.id === id)?.builtinTime
    return builtin || time
  }

  const submit = () => {
    // 只把有值的覆盖传上去：空串在 payload 里表示「撤销覆盖」，与「没改」不同，
    // 所以这里逐项过滤后再交（后端也认空串，但少传一层更清楚）
    const overrides: Record<string, string> = {}
    for (const [id, value] of Object.entries(providerTimes)) {
      if (value) overrides[id] = value
    }
    void saveScheduleEntry({
      ...(isNew ? {} : { id: entry.id }),
      label: label.trim(),
      time: time.trim(),
      enabled: entry ? entry.enabled : true,
      providers: options.map(item => item.id).filter(id => providers.includes(id)),
      providerTimes: overrides,
      retryTime: retryTime.trim(),
      retryGapMinutes: Number(retryGap) || 0,
      retryLimit: Number(retryLimit) || 0,
    })
  }

  return (
    <div className="ck-sched-editor">
      <div className="ck-set-line">
        <span className="ck-set-k">名称</span>
        <span className="ck-set-v">
          <Input
            type="text"
            className="ck-sched-name"
            placeholder="可选 · 缺省显示时刻"
            value={label}
            disabled={store.saving}
            onChange={event => setLabel(event.currentTarget.value)}
          />
        </span>
      </div>

      <div className="ck-set-line">
        <span className="ck-set-k">每天时刻</span>
        <span className="ck-set-v ck-set-row">
          <Input
            type="time"
            className="ck-sched-time-input font-mono tabular-nums"
            value={time}
            disabled={store.saving}
            onChange={event => setTime(event.currentTarget.value)}
          />
          <span className="ck-sub">北京时间 · 错过时点开机后会自动补签</span>
        </span>
      </div>

      <div className="ck-set-line ck-set-line-top">
        <span className="ck-set-k">签到范围</span>
        <span className="ck-set-v">
          <div className="ck-mini-row">
            <span className="ck-mini-note">
              到点时逐个签到，已勾 {providers.length} / {options.length} 家
            </span>
            <Button size='default' variant='ghost'
              type="button"
              className="ck-mini-btn"
              disabled={store.saving}
              onClick={() => setProviders(options.map(item => item.id))}
            >
              全选
            </Button>
            <Button size='default' variant='ghost'
              type="button"
              className="ck-mini-btn"
              disabled={store.saving || providers.length === 0}
              onClick={() => setProviders([])}
            >
              清空
            </Button>
          </div>
          <div className="ck-range">
            {options.map(option => (
              <Label className="ck-range-item" key={option.id}>
                <Checkbox
                  checked={providers.includes(option.id)}
                  disabled={store.saving}
                  onCheckedChange={checked => toggleProvider(option.id, checked === true)}
                />
                <span className="ck-range-name" title={option.label}>{option.label}</span>
              </Label>
            ))}
          </div>
          {providers.length === 0
            ? <div className="ck-mini-warn">至少勾一家才能保存（不勾等于这条条目不签到）</div>
            : null}
        </span>
      </div>

      {/* ── 逐家时刻 ────────────────────────────────────────────
          上游的每日窗口各不相同（Qoder 10:00、ZCode 19:00），所以每家可以
          单独定时刻；不填就用内置的专属时刻，再没有就跟「每天时刻」。 */}
      <div className="ck-set-line ck-set-line-top">
        <span className="ck-set-k">各家时刻</span>
        <span className="ck-set-v">
          <div className="ck-mini-note">
            上游窗口不同的家各自定时，留空 = 用内置专属时刻（没有就跟上面的「每天时刻」）
          </div>
          <div className="ck-ptimes">
            {options.filter(option => providers.includes(option.id)).map(option => {
              const override = providerTimes[option.id] ?? ''
              const builtin = option.builtinTime ?? ''
              const effective = effectiveTime(option.id)
              return (
                <div className="ck-ptime-row" key={option.id}>
                  <span className="ck-ptime-name" title={option.label}>{option.label}</span>
                  <Input
                    type="time"
                    className="ck-sched-time-input font-mono tabular-nums"
                    value={override || effective}
                    disabled={store.saving}
                    onChange={event => {
                      const next = event.currentTarget.value
                      setProviderTimes(prev => ({ ...prev, [option.id]: next }))
                    }}
                  />
                  {override
                    ? <Button size='default' variant='ghost' className="ck-mini-btn"
                        disabled={store.saving}
                        title="撤销自定义，回到内置 / 条目时刻"
                        onClick={() => setProviderTimes(prev => {
                          const next = { ...prev }
                          delete next[option.id]
                          return next
                        })}
                      >撤销</Button>
                    : <span className="ck-ptime-tag">
                        {builtin ? `专属 ${builtin}` : '跟随条目'}
                      </span>}
                </div>
              )
            })}
          </div>
        </span>
      </div>

      {/* ── 补签与重试 ──────────────────────────────────────────
          补签轮：主轮之后再看一眼「今天还有谁没签上」，只补那几个。
          重试：中性未领取（上游说窗口没开 / 暂未下发）的账号隔多久再试、试几次。 */}
      <div className="ck-set-line">
        <span className="ck-set-k">补签时刻</span>
        <span className="ck-set-v ck-set-row">
          <Input
            type="time"
            className="ck-sched-time-input font-mono tabular-nums"
            value={retryTime}
            disabled={store.saving}
            onChange={event => setRetryTime(event.currentTarget.value)}
          />
          <span className="ck-sub">
            只补今天还没签上的账号 · 清空 = 关闭补签 · 各家按「主轮时刻 + 上面的重试间隔」顺延
            （Qoder 10:11、ZCode 19:11）
          </span>
        </span>
      </div>

      <div className="ck-set-line">
        <span className="ck-set-k">窗口重试</span>
        <span className="ck-set-v ck-set-row">
          <Input
            type="number"
            min={0}
            max={120}
            className="ck-sched-num"
            value={retryGap}
            disabled={store.saving}
            onChange={event => setRetryGap(event.currentTarget.value)}
          />
          <span className="ck-sub">分钟后重试 · 最多</span>
          <Input
            type="number"
            min={0}
            max={20}
            className="ck-sched-num"
            value={retryLimit}
            disabled={store.saving}
            onChange={event => setRetryLimit(event.currentTarget.value)}
          />
          <span className="ck-sub">次 · 填 0 关闭重试（只对「窗口没开 / 暂未下发」的账号生效）</span>
        </span>
      </div>

      <div className="ck-sched-actions">
        <Button size="sm" variant="outline" disabled={store.saving} onClick={() => patch({ editingId: null })}>
          取消
        </Button>
        <Button size="sm" variant="default" disabled={!canSave} onClick={submit}>
          {store.saving ? '保存中…' : '保存'}
        </Button>
      </div>
    </div>
  )
}

/* ─── 条目行 ───────────────────────────────── */

/** 「上次执行」那一行：`时间 · 原因 —— 成功 N · 保活 N · 失败 N` */
function lastResultLine(entry: ScheduleEntry): string {
  const result: CheckinResult | null = entry.lastResult
  if (!result || typeof result !== 'object') return '还没执行过'
  const count = (key: keyof CheckinResult) => Number(result[key]) || 0
  const parts = [`成功 ${count('succeeded')}`]
  if (count('active') > 0) parts.push(`保活 ${count('active')}`)
  if (count('failedCount') > 0) parts.push(`失败 ${count('failedCount')}`)
  const at = result.at ? formatTime(result.at) : '—'
  const reason = result.reason ? ` · ${result.reason}` : ''
  return `${at}${reason} —— ${parts.join(' · ')}`
}

/** 范围的摘要（全选时说「全部 N 家」，不把十几个名字串起来） */
function scopeSummary(entry: ScheduleEntry, options: { id: string; label: string }[]): string {
  const lookup = new Map(options.map(item => [item.id, item.label]))
  const names = entry.providers.map(id => lookup.get(id) ?? id)
  if (names.length === 0) return '未勾选提供商'
  if (options.length > 0 && names.length >= options.length) return `全部 ${options.length} 家`
  return names.join('、')
}

function EntryRow({ entry, options, masterOn, defaultRetryTime }: {
  entry: ScheduleEntry
  options: { id: string; label: string; builtinTime?: string | null }[]
  masterOn: boolean
  defaultRetryTime: string
}): React.ReactElement {
  const editing = store.editingId === entry.id
  const running = store.runningScheduleId === entry.id
  const nextRun = Number(entry.nextRunAt)
  const nextText = masterOn && entry.enabled && Number.isFinite(nextRun) && nextRun > 0
    ? `下次 ${formatClock(nextRun)}`
    : '—'

  const fullScope = entry.providers
    .map(id => options.find(item => item.id === id)?.label ?? id)
    .join('、') || '未勾选提供商'

  // ── 今天的状态：哪些家跑过了、哪些还没到点 ──────────────────
  // 改造前这里只有一个「今天已签 / 未签」，看不到「Qoder 要等到 10:01」这类
  // 事实 —— 而那正是用户最需要知道的一格（设的 00:01，为什么没签）。
  const rows = entry.providerRows ?? []
  // 三档状态必须分开说，否则用户分不清「卡住了」与「在等窗口」：
  //   待执行 = 到点了还没跑；等窗口 = 今天还没到点；其余是今天已跑过。
  const pending = rows.filter(row => !row.firedToday)
  const overdue = pending.filter(row => row.overdue === true)
  const waiting = pending.filter(row => row.overdue !== true)
  const statusText = rows.length === 0
    ? '—'
    : [
        overdue.length > 0
          ? `待执行 ${overdue.map(row => `${row.label} ${row.time}`).join('、')}`
          : '',
        waiting.length > 0
          ? `等窗口 ${waiting.map(row => `${row.label} ${row.time}`).join('、')}`
          : '',
        pending.length === 0 ? '今天都跑过了' : '',
      ].filter(Boolean).join(' · ')
  // 补签时刻（空串 = 关闭）：界面明确说清「几点补、补谁」。
  // 补签点按各家主轮顺延（Qoder 10:11、ZCode 19:11），所以列出逐家的时刻 ——
  // 只显示一个 00:10 会让人以为那几家也在 00:10 补。
  const retryTimes = entry.providerRetryTimes ?? {}
  const retryDetail = Object.keys(retryTimes).length > 0
    ? `（${Object.entries(retryTimes)
        .map(([id, time]) => `${options.find(item => item.id === id)?.label ?? id} ${time}`)
        .join(' / ')}）`
    : ''
  const retryText = entry.retryTime
    ? `补签 ${retryDetail} 只补今天没签上的`
    : '补签已关闭'
  // 等窗口重试的账号数：有值就说明「还会再试」，不是一次失败就结束
  const retryCount = Number(entry.retryCount) || 0

  return (
    <div className={editing ? 'ck-sched-row open' : 'ck-sched-row'}>
      <div className="ck-sched-main">
        <Switch
          checked={entry.enabled}
          disabled={store.saving}
          // 只改开关：其余字段原样回传（后端 upsert 是全量覆盖）。
          // 不收表单、不弹提示 —— 这是行上的快捷开关，不是「编辑完成」。
          onCheckedChange={checked => void saveScheduleEntry({
            id: entry.id,
            label: entry.label ?? '',
            enabled: checked === true,
            time: entry.time ?? '',
            providers: Array.isArray(entry.providers) ? entry.providers : [],
          }, { closeEditor: false, silent: true })}
        />
        <div className="ck-prov-info">
          <div className="ck-prov-name">
            {entry.displayLabel || entry.time || entry.id}
            {entry.enabled ? null : <Badge variant="outline" shape="tag">已停用</Badge>}
          </div>
          <div className="ck-prov-desc ck-sched-scope" title={fullScope}>
            基准 {entry.time || '—'} · {scopeSummary(entry, options)}
            {rows.length > 0
              ? ` · 各家时刻 ${rows.map(row => `${row.label} ${row.time}`).join(' / ')}`
              : ''}
          </div>
          <div className="ck-prov-desc">
            {statusText} · {retryText}
            {retryCount > 0 ? ` · ${retryCount} 个账号等窗口重试` : ''}
          </div>
          <div className="ck-prov-desc">上次执行：{lastResultLine(entry)}</div>
        </div>
        <div className="ck-prov-right">
          <span className="ck-sched-next">{nextText}</span>
          <Button
            size="sm"
            variant="ghost"
            disabled={running || store.runningScheduleId !== null}
            onClick={() => void runScheduleEntry(entry.id)}
          >
            {running ? '执行中…' : '立即执行'}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => patch({ editingId: editing ? null : entry.id })}
          >
            {editing ? '收起' : '编辑'}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="text-destructive"
            disabled={store.saving}
            onClick={async () => {
              const name = entry.displayLabel || entry.time || entry.id
              if (!await confirmDialog(`确定删除「${name}」？删除后到点不再执行。`)) return
              void removeScheduleEntry(entry.id)
            }}
          >
            删除
          </Button>
        </div>
      </div>
      {editing ? <EditorForm entry={entry} options={options} defaultRetryTime={defaultRetryTime} /> : null}
    </div>
  )
}

/* ─── 保活模型链（国际版）──────────────────── */

/**
 * 拖动排序：模块级状态。
 *
 * 拖动期间**只动 DOM**（`insertBefore`），松手才把最终顺序写进 state。
 * 每次移动都写 state 会让 React 重渲、行节点被替换，手里那个节点变成孤儿
 * （`getBoundingClientRect()` 全 0），后续落点判定全错 —— 这是旧 iframe 版
 * 真实踩过的坑。
 */
type DragState = {
  container: HTMLElement
  row: HTMLElement
  models: string[]
  onChange: (next: string[]) => void
}

let drag: DragState | null = null

function insertIndex(rows: HTMLElement[], clientY: number): number {
  for (let index = 0; index < rows.length; index += 1) {
    const rect = rows[index].getBoundingClientRect()
    if (clientY < rect.top + rect.height / 2) return index
  }
  return rows.length
}

function onPointerMove(event: PointerEvent): void {
  if (!drag) return
  if (event.buttons === 0) { finishDrag(); return }
  const rows = [...drag.container.querySelectorAll<HTMLElement>('.ck-chain-row')]
    .filter(row => row !== drag!.row)
  const reference = rows[insertIndex(rows, event.clientY)] ?? null
  if (reference === drag.row.nextElementSibling) return
  if (reference === null && drag.row === drag.container.lastElementChild) return
  drag.container.insertBefore(drag.row, reference)
}

function finishDrag(): void {
  const state = drag
  drag = null
  window.removeEventListener('pointermove', onPointerMove)
  window.removeEventListener('pointerup', finishDrag)
  window.removeEventListener('pointercancel', finishDrag)
  document.body.classList.remove('ck-chain-dragging')
  if (!state) return
  state.row.classList.remove('dragging')
  // 行上的 `data-chain-index` 记的是**原数组**下标：按 DOM 顺序读回来就是新顺序
  const order = [...state.container.querySelectorAll<HTMLElement>('.ck-chain-row')]
    .map(row => Number(row.dataset.chainIndex))
    .filter(index => Number.isInteger(index))
    .map(index => state.models[index])
  const changed = order.length === state.models.length
    && order.some((model, index) => model !== state.models[index])
  if (changed) state.onChange(order)
}

function startDrag(event: React.PointerEvent<HTMLSpanElement>, index: number, models: string[], onChange: (next: string[]) => void): void {
  if (event.button !== 0 || drag) return
  if (models.length < 2) return
  const row = event.currentTarget.closest<HTMLElement>('.ck-chain-row')
  const container = row?.parentElement
  if (!row || !container) return
  event.preventDefault()
  drag = { container, row, models: [...models], onChange }
  row.classList.add('dragging')
  document.body.classList.add('ck-chain-dragging')
  window.addEventListener('pointermove', onPointerMove)
  window.addEventListener('pointerup', finishDrag)
  window.addEventListener('pointercancel', finishDrag)
}

/** 拖动把手（六点两列，与账号页顺序拖拽同一枚） */
function GripIcon(): React.ReactElement {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" aria-hidden="true">
      <circle cx="9" cy="6" r="1.5" /><circle cx="15" cy="6" r="1.5" />
      <circle cx="9" cy="12" r="1.5" /><circle cx="15" cy="12" r="1.5" />
      <circle cx="9" cy="18" r="1.5" /><circle cx="15" cy="18" r="1.5" />
    </svg>
  )
}

function KeepalivePanel({ center }: { center: CheckinCenter }): React.ReactElement {
  const keyState = useModelApiKeys(() => callJson('GET', '/api/model-access/keys'))
  const current = center.keepalive
  const models = store.keepaliveDraft ?? current.models
  const defaults = current.defaultModels ?? []
  const dirty = store.keepaliveDraft !== null
    && (store.keepaliveDraft.length !== current.models.length
      || store.keepaliveDraft.some((item, index) => item !== current.models[index]))
  const [input, setInput] = React.useState('')

  const setDraft = (next: string[]) => patch({ keepaliveDraft: next })

  const add = () => {
    const name = input.trim()
    if (!name) return
    if (models.includes(name)) return
    setDraft([...models, name])
    setInput('')
  }

  return (
    <Card render={<section />} className="panel ck-chain-panel">
      <CardHeader className="panel-head">
        <h2>保活模型链</h2>
        <span className="panel-sub">WorkBuddy 国际版日活任务的免费模型 · 全局一份，与条目无关</span>
        <Button
          size="sm"
          variant="default"
          className="ml-auto"
          disabled={!dirty || store.saving}
          onClick={() => void saveKeepaliveModels(models)}
        >
          {store.saving ? '保存中…' : '保存'}
        </Button>
      </CardHeader>
      <div className="ck-panel-pad">
        <ModelApiKeyField {...keyState} value={current.apiKeyId ?? null} disabled={store.saving}
          onChange={apiKeyId => { void saveKeepaliveModels(models, apiKeyId) }}
          onRefresh={() => { void keyState.refresh().catch(() => {}) }} t={t} id='keepalive-api-key' />
        {current.apiKeyDefaulted
          ? <p className='text-xs leading-[1.6] text-subtle'>
              {t('默认使用第一把可用 API Key，想换就在上面选一把')}
            </p>
          : null}
        {current.authError ? <p role='status' className='text-xs text-destructive'>{current.authError}</p> : null}
        <div className="ck-set-line ck-set-line-top">
          <span className="ck-set-k">模型清单</span>
          <span className="ck-set-v">
            <div className="ck-mini-row">
              <span className="ck-mini-note">按序逐个尝试，共 {models.length} 个</span>
              <Button size='default' variant='ghost'
                type="button"
                className="ck-mini-btn"
                disabled={store.saving || defaults.length === 0}
                title="把缺省链填进清单，再点「保存」生效"
                onClick={() => setDraft([...defaults])}
              >
                填入缺省链
              </Button>
            </div>
            <div className="ck-chain">
              {models.map((model, index) => (
                <div className="ck-chain-row" key={model} data-chain-index={index}>
                  <span
                    className="ck-chain-grip"
                    role="button"
                    aria-label="按住拖动调整顺序"
                    title="按住拖动，调整这个模型在链里的顺序"
                    onPointerDown={event => startDrag(event, index, models, setDraft)}
                  >
                    <GripIcon />
                  </span>
                  <span className="ck-chain-name" title={model}>{model}</span>
                  <span className="ck-chain-index">#{index + 1}</span>
                  <Button size='default' variant='ghost'
                    type="button"
                    className="ck-chain-del"
                    disabled={store.saving}
                    aria-label={`移除 ${model}`}
                    title="从清单里移除"
                    onClick={() => setDraft(models.filter((_, keep) => keep !== index))}
                  >
                    ×
                  </Button>
                </div>
              ))}
              {models.length === 0
                ? <Empty className="ck-chain-empty">清单为空 —— 保存后恢复缺省链</Empty>
                : null}
            </div>
            <div className="ck-chain-add">
              <Input
                type="text"
                className="ck-chain-input font-mono text-xs"
                placeholder="WorkBuddy 国际版保活模型，按顺序回退"
                value={input}
                disabled={store.saving}
                onChange={event => setInput(event.currentTarget.value)}
                onKeyDown={event => {
                  if (event.key !== 'Enter') return
                  event.preventDefault()
                  add()
                }}
              />
              <Button size="sm" variant="outline" disabled={store.saving || input.trim() === ''} onClick={add}>
                添加模型
              </Button>
              {dirty ? <span className="ck-chain-dirty">有未保存的改动</span> : null}
            </div>
          </span>
        </div>
        <div className="ck-note">
          「保活模型链」是 WorkBuddy 国际版领日活时用来保活的免费模型，按顺序逐个尝试、
          第一个成功即止；清空清单并保存 = 恢复缺省链（当前缺省：
          {defaults.length ? defaults.join('、') : '（无）'}）。
          当天去重 + 幂等领取：重复执行只会拿到「已领取」，不会重复加分。
        </div>
      </div>
    </Card>
  )
}

/* ─── 猫猫旅行排程（国内版）────────────────── */

const TRAVEL_TIP = '每天按这里的时刻自动巡检一遍 WorkBuddy 国内版账号：没有猫就领养、'
  + '在家就派出、到站就领奖。一趟只推进一步，所以建议留两个时刻'
  + '（早上派出、晚上领奖）。企业版与国际版账号自动跳过。'

function TravelPanel({ center }: { center: CheckinCenter }): React.ReactElement {
  const travel = center.travel
  const times = travel.times ?? []
  const fired = travel.firedToday ?? []
  const draft = store.travelTimesDraft
  const text = draft ?? times.join(', ')
  const dirty = draft !== null && draft.replace(/\s+/g, '') !== times.join(',')
  const last = travel.lastResult
  const actions = Array.isArray(last?.actions) ? last.actions : []
  const isPaused = travel.available === false
  const failed = Number(last?.failed) || 0

  const submit = () => {
    const list = String(text)
      .split(/[,，\s]+/)
      .map(item => item.trim())
      .filter(Boolean)
    void saveTravel({ times: list })
  }

  return (
    <Card render={<section />} className="panel">
      <CardHeader className="panel-head">
        <h2>猫猫旅行定时排程</h2>
        {/* 问号提示：宿主有 desktop-tooltip.tsx，`data-tip` 是它的契约 —— 迁到宿主后
            这条提示才真正长得和别的页一样（iframe 里拿不到那个脚本） */}
        <span className="tip-q" data-tip={TRAVEL_TIP} />
        <span className="panel-sub">每天定点巡检 · 一趟推进一步 · 与签到排程互相独立</span>
        <Button
          size="sm"
          variant="outline"
          className="ml-auto flex-none"
          disabled={travel.running || isPaused || store.saving}
          onClick={() => void runTravelNow()}
        >
          {travel.running ? '巡检中…' : '立即巡检全部'}
        </Button>
      </CardHeader>
      <div className="ck-panel-pad">
        {isPaused ? (
          <div className="ck-note">
            {t('自动巡检已暂停：{reason}', { reason: travel.pausedReason ?? '' })}
          </div>
        ) : null}
        <div className="ck-set-line">
          <span className="ck-set-k">开关</span>
          <span className="ck-set-v ck-set-row">
            <Switch
              checked={travel.enabled}
              disabled={store.saving}
              onCheckedChange={checked => void saveTravel({ enabled: checked === true })}
            />
            <span className="ck-sub">
              {travel.enabled ? '开着：按下面的时刻自动巡检' : '关着：不自动巡检（仍可手动巡检）'}
            </span>
          </span>
        </div>
        <div className="ck-set-line">
          <span className="ck-set-k">时刻</span>
          <span className="ck-set-v ck-set-row">
            <Input
              className="ck-travel-times"
              placeholder="09:00, 21:00"
              value={text}
              disabled={store.saving}
              onChange={event => patch({ travelTimesDraft: event.currentTarget.value })}
            />
            <Button size="sm" variant="outline" disabled={!dirty || store.saving} onClick={submit}>
              保存
            </Button>
          </span>
        </div>
        <div className="ck-note">
          北京时间 · 逗号分隔 · 最多 6 条 · 到点后最多晚 30 秒执行；错过时点开机后会补跑一轮。
          {fired.length ? ` 今天已执行：${fired.join(' / ')}` : ''}
        </div>
        {last && (actions.length > 0 || last.error) ? (
          <div className="ck-note">
            {last.error ? t('巡检失败：{reason}', { reason: last.error }) : (
              <>
                上一轮（{last.at ? formatTime(last.at) : ''}）：{actions.length} 个动作，+{Number(last.credit) || 0} 积分
                {failed > 0 ? ` · ${t('失败 {n}', { n: failed })}` : ''}
                {actions.slice(0, 6).map(action =>
                  ` · ${action.name ?? action.accountId ?? ''}: ${action.message ?? action.outcome ?? ''}`).join('')}
              </>
            )}
          </div>
        ) : null}
      </div>
    </Card>
  )
}

/* ─── 主体 ─────────────────────────────────── */

const SCHEDULE_TIP = '到点后自动签到勾选提供商的可用账号。各家签到接口都是幂等的，'
  + '重复执行不会重复领取。错过时点开机后会自动补签，不会因为当时没开机而漏掉。'
  + '上游每日窗口不同的家（Qoder 10:00、ZCode 19:00）按各自的专属时刻签，'
  + '可以在条目里单独改；补签时刻那一轮只补今天还没签上的账号。'

export function ScheduleTab({ center }: { center: CheckinCenter }): React.ReactElement {
  const auto = center.auto
  const options = auto?.providerOptions ?? []
  const entries = auto?.schedules ?? []
  const defaultRetryTime = auto?.defaultRetryTime ?? '00:10'
  const panels = new Set(center.adminPanels ?? [])
  const keepalive = panels.has('daily_keepalive') ? <KeepalivePanel center={center} /> : null
  const travel = panels.has('growth_tasks') ? <TravelPanel center={center} /> : null
  const bothRegions = keepalive !== null && travel !== null

  return (
    <div className="ck-main">
      <Card render={<section />} className="panel">
        <CardHeader className="panel-head">
          <h2>定时签到</h2>
          <span className="tip-q" data-tip={SCHEDULE_TIP} />
          <span className="panel-sub">每条条目各有时刻与范围 · 各家按自己的窗口时刻签 · 补签轮只补未签到的账号</span>
          <Button
            size="sm"
            variant="outline"
            className="ml-auto flex-none"
            disabled={store.saving}
            onClick={() => patch({ editingId: store.editingId === 'new' ? null : 'new' })}
          >
            {store.editingId === 'new' ? '收起新建' : '添加条目'}
          </Button>
        </CardHeader>
        <div className="ck-panel-pad">
          <div className="ck-set-line">
            <span className="ck-set-k">总开关</span>
            <span className="ck-set-v ck-set-row">
              <Switch
                checked={auto?.enabled === true}
                disabled={store.saving}
                onCheckedChange={checked => void toggleMaster(checked === true)}
              />
              <span className="ck-sub">
                {auto?.enabled ? '开着：启用的条目按各自时刻执行' : '关着：所有条目都不执行'}
              </span>
            </span>
          </div>
        </div>
        <div>
          {/* 新建行落在列表首位：点「添加条目」后视线停在卡头，新行跟着出现在
              它下面才接得上（落到底部时用户会以为没反应） */}
          {store.editingId === 'new' ? (
            <div className="ck-sched-row open ck-sched-row-new">
              <div className="ck-sched-main">
                <span className="ck-new-dot" aria-hidden="true">＋</span>
                <div className="ck-prov-info">
                  <div className="ck-prov-name">新建定时条目</div>
                  <div className="ck-prov-desc">
                    填好时刻与签到范围后点「保存」；名称可以留空（界面显示时刻）
                  </div>
                </div>
              </div>
              <EditorForm entry={null} options={options} defaultRetryTime={defaultRetryTime} />
            </div>
          ) : null}
          {entries.map(entry => (
            <EntryRow key={entry.id} entry={entry} options={options}
              masterOn={auto?.enabled === true} defaultRetryTime={defaultRetryTime} />
          ))}
          {entries.length === 0 && store.editingId !== 'new' ? (
            <Empty className="ck-empty">
              还没有定时条目 —— 点「添加条目」建一条，或到「签到中心」手动签到。
            </Empty>
          ) : null}
        </div>
      </Card>

      {/* 两块地区专属面板：由插件申报的 adminPanels 决定。
          国内版与国际版默认都启用，所以两块**会同时出现** —— 那种情况收进
          同一行分栏（左保活模型链 / 右猫猫旅行排程）；各自独占整行时，每张卡
          的右半截都是空白。只有一块时不套容器：模型链仍收半幅。 */}
      {bothRegions ? (
        <div className="ck-region-row">{keepalive}{travel}</div>
      ) : (
        <>
          {keepalive}
          {travel}
        </>
      )}
    </div>
  )
}
