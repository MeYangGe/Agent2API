import { useState } from 'react'
import {
  Button, Checkbox, Input, Sheet, SheetBody, SheetContent, SheetFooter,
  SheetHeader, SheetTitle, Switch,
  Card, Label } from '@ui'
import { Empty, SectionHead } from '../../components/ui'
import { store } from '../../state/store'
import {
  deleteCheckinSchedule, runCheckinSchedule, saveAutoCheckin, saveCheckinSchedule,
  type CheckinCenter, type ScheduleEntry,
} from '../../api/checkin'
import { checkinSummary } from '../../lib/checkin'
import { formatStamp } from '../../lib/format'
import type { CheckinAction } from '../CheckinPage'
import { CheckinRegionSettings } from './CheckinRegionSettings'
import { t } from '../../i18n'

export function CheckinSchedules({ center, busy, action }: {
  center: CheckinCenter
  busy: string
  action: CheckinAction
}) {
  const [editing, setEditing] = useState<ScheduleEntry | 'new' | null>(null)
  const [remove, setRemove] = useState<ScheduleEntry | null>(null)
  const auto = center.auto
  const options = auto?.providerOptions || []
  const entries = auto?.schedules || []

  const toggle = (entry: ScheduleEntry, enabled: boolean) => {
    void action(`toggle:${entry.id}`, () => saveCheckinSchedule({
      id: entry.id, label: entry.label, enabled, time: entry.time, providers: entry.providers,
    }), t('已保存'))
  }

  const run = async (id: string) => {
    const result = await action(`run:${id}`, () => runCheckinSchedule(id))
    if (result) {
      const summary = checkinSummary(result)
      store.toast(summary.text, summary.tone)
    }
  }

  return (
    <>
      <div className='m-section' style={{ marginTop: 0 }}>
        <SectionHead title={t('定时签到')} />
        <Card className='m-card m-card-pad'>
          <div className='m-ck-toolbar'>
            <div className='m-ck-grow'>
              <div className='m-ck-title'>{t('总开关')}</div>
              <div className='m-ck-note'>{auto?.enabled ? t('启用的条目按各自时刻执行') : t('所有定时签到条目均已暂停')}</div>
            </div>
            <Switch checked={auto?.enabled === true} disabled={Boolean(busy)} aria-label={t('定时签到总开关')}
              onCheckedChange={enabled => void action('master', () => saveAutoCheckin(enabled), t('已保存'))} />
          </div>
          <p className='m-ck-note'>{t('北京时间 · 错过时点开机后会自动补签。任务在网关执行，关闭手机不影响排程。')}</p>
          <Button size='sm' variant='outline' disabled={Boolean(busy) || !options.length} onClick={() => setEditing('new')}>
            {t('添加条目')}
          </Button>
        </Card>
      </div>

      {entries.map(entry => (
        <div key={entry.id} className='m-section'>
          <Card className='m-card m-card-pad'>
            <div className='m-ck-toolbar'>
              <div className='m-ck-grow'>
                <div className='m-ck-title'>{entry.displayLabel || entry.label || entry.time}</div>
                <div className='m-ck-note'>{t('每天 {time}', { time: entry.time })}</div>
              </div>
              <Switch checked={entry.enabled} disabled={Boolean(busy)}
                aria-label={entry.displayLabel || entry.time} onCheckedChange={enabled => toggle(entry, enabled)} />
            </div>
            <p className='m-ck-note'>{entry.providers.map(id => t(options.find(item => item.id === id)?.label || id)).join('、')}</p>
            {/* 各家按自己的窗口时刻签（Qoder 10:01 / ZCode 19:01），
                这一行说明「还没到点的是哪几家」——手机上最需要看到的一格 */}
            {(entry.providerRows || []).length
              ? <p className='m-ck-note'>
                  {entry.providerRows.map(row => `${t(row.label)} ${row.time}${row.firedToday ? t('（已跑）') : ''}`).join(' · ')}
                  {entry.retryTime ? t(' · 补签 {time}', { time: entry.retryTime }) : ''}
                  {Number(entry.retryCount) > 0 ? t(' · {n} 个账号等窗口重试', { n: entry.retryCount }) : ''}
                </p>
              : null}
            <p className='m-ck-note'>{entry.nextRunAt ? t('下次执行：{time}', { time: formatStamp(entry.nextRunAt) })
              : t('未安排下次执行')}
              {entry.lastResult?.at ? t(' · 上次执行：{time}', { time: formatStamp(entry.lastResult.at) }) : ''}</p>
            {entry.lastResult ? <p className='m-ck-note'>{checkinSummary(entry.lastResult).text}</p> : null}
            <div className='m-ck-actions'>
              <Button size='sm' variant='outline' disabled={Boolean(busy) || auto?.running} onClick={() => void run(entry.id)}>
                {busy === `run:${entry.id}` ? t('执行中…') : t('立即执行')}
              </Button>
              <Button size='sm' variant='ghost' disabled={Boolean(busy)} onClick={() => setEditing(entry)}>{t('编辑')}</Button>
              <Button size='sm' variant='ghost' disabled={Boolean(busy)} className='text-destructive' onClick={() => setRemove(entry)}>{t('删除')}</Button>
            </div>
          </Card>
        </div>
      ))}
      {!entries.length ? <Empty text={t('还没有定时条目，添加一条或到签到中心手动签到')} /> : null}

      <CheckinRegionSettings center={center} busy={busy} action={action} />

      {editing ? <ScheduleEditor key={typeof editing === 'string' ? editing : editing.id}
        entry={editing === 'new' ? null : editing} options={options} busy={busy} action={action} onClose={() => setEditing(null)} /> : null}
      <Sheet open={Boolean(remove)} onOpenChange={(open, details) => {
        if (open) return
        if (busy) { details.cancel(); return }
        setRemove(null)
      }}>
        <SheetContent showCloseButton={!busy}>
          <SheetHeader><SheetTitle>{t('删除定时签到条目')}</SheetTitle></SheetHeader>
          <SheetBody><p className='m-ck-note'>{t('确定删除「{name}」？删除后到点不再执行。', {
            name: remove?.displayLabel || remove?.time || '',
          })}</p></SheetBody>
          <SheetFooter>
            <Button variant='outline' disabled={Boolean(busy)} onClick={() => setRemove(null)}>{t('取消')}</Button>
            <Button variant='destructive' disabled={Boolean(busy)} onClick={async () => {
              if (!remove) return
              const result = await action(`delete:${remove.id}`, () => deleteCheckinSchedule(remove.id), t('已删除'))
              if (result !== undefined) setRemove(null)
            }}>{busy ? t('删除中…') : t('删除')}</Button>
          </SheetFooter>
        </SheetContent>
      </Sheet>
    </>
  )
}

function ScheduleEditor({ entry, options, busy, action, onClose }: {
  entry: ScheduleEntry | null
  options: { id: string; label: string; builtinTime?: string | null }[]
  busy: string
  action: CheckinAction
  onClose: () => void
}) {
  const [label, setLabel] = useState(entry?.label || '')
  const [time, setTime] = useState(entry?.time || '00:01')
  const [providers, setProviders] = useState(entry?.providers || options.map(item => item.id))
  // 逐家时刻覆盖（空串 = 撤销）与补签轮：与桌面端同一套字段
  const [providerTimes, setProviderTimes] = useState<Record<string, string>>(entry?.providerTimes || {})
  const [retryTime, setRetryTime] = useState(entry?.retryTime ?? '00:10')
  const [retryGap, setRetryGap] = useState(String(entry?.retryGapMinutes ?? 10))
  const [retryLimit, setRetryLimit] = useState(String(entry?.retryLimit ?? 3))
  const unavailable = providers.filter(id => !options.some(item => item.id === id))
  const available = [...options, ...unavailable.map(id => ({ id, label: id }))]

  const submit = async () => {
    if (!/^([01]\d|2[0-3]):[0-5]\d$/.test(time) || !providers.length) {
      store.toast(t('请填写合法时刻并至少选择一家提供商'), 'err')
      return
    }
    const overrides: Record<string, string> = {}
    for (const [id, value] of Object.entries(providerTimes)) {
      if (value) overrides[id] = value
    }
    const result = await action('schedule-save', () => saveCheckinSchedule({
      ...(entry ? { id: entry.id } : {}), label: label.trim(), time,
      enabled: entry ? entry.enabled : true, providers,
      providerTimes: overrides,
      retryTime: retryTime.trim(),
      retryGapMinutes: Number(retryGap) || 0,
      retryLimit: Number(retryLimit) || 0,
    }), t('已保存'))
    if (result) onClose()
  }

  return (
    <Sheet open onOpenChange={(open, details) => {
      if (open) return
      if (busy) { details.cancel(); return }
      onClose()
    }}>
      <SheetContent showCloseButton={!busy}>
        <SheetHeader><SheetTitle>{entry ? t('编辑定时条目') : t('新建定时条目')}</SheetTitle></SheetHeader>
        <SheetBody>
          <Label className='m-ck-field'>{t('名称（可选）')}
            <Input className='text-base' value={label} disabled={Boolean(busy)} onChange={event => setLabel(event.currentTarget.value)} />
          </Label>
          <Label className='m-ck-field'>{t('每天时刻（北京时间）')}
            <Input type='time' className='text-base' value={time} disabled={Boolean(busy)} onChange={event => setTime(event.currentTarget.value)} />
          </Label>
          <div className='m-ck-title'>{t('各家时刻')}</div>
          <p className='m-ck-note'>{t('上游窗口不同的家各自定时，留空 = 用内置专属时刻（没有就跟上面的每天时刻）')}</p>
          {available.filter(option => providers.includes(option.id)).map(option => {
            const builtin = options.find(item => item.id === option.id)?.builtinTime || ''
            const value = providerTimes[option.id] || ''
            return (
              <Label className='m-ck-field' key={`time-${option.id}`}>
                {t(option.label)}{builtin ? t('（专属 {time}）', { time: builtin }) : ''}
                <Input type='time' className='text-base' value={value || builtin || time}
                  disabled={Boolean(busy)}
                  onChange={event => setProviderTimes(prev => ({ ...prev, [option.id]: event.currentTarget.value }))} />
              </Label>
            )
          })}
          <Label className='m-ck-field'>{t('补签时刻（只补今天没签上的账号，清空 = 关闭）')}
            <Input type='time' className='text-base' value={retryTime} disabled={Boolean(busy)}
              onChange={event => setRetryTime(event.currentTarget.value)} />
          </Label>
          <div className='m-ck-toolbar'>
            <Label className='m-ck-grow'>{t('窗口重试间隔（分钟，0 = 关闭）')}
              <Input type='number' className='text-base' value={retryGap} disabled={Boolean(busy)}
                onChange={event => setRetryGap(event.currentTarget.value)} />
            </Label>
            <Label className='m-ck-grow'>{t('最多重试次数（0 = 关闭）')}
              <Input type='number' className='text-base' value={retryLimit} disabled={Boolean(busy)}
                onChange={event => setRetryLimit(event.currentTarget.value)} />
            </Label>
          </div>
          <div className='m-ck-title'>{t('签到范围')}</div>
          {available.map(option => <Label className='m-ck-toolbar' key={option.id}>
            <Checkbox checked={providers.includes(option.id)} disabled={Boolean(busy)} onCheckedChange={checked => {
              setProviders(list => checked ? [...new Set([...list, option.id])] : list.filter(id => id !== option.id))
            }} />
            <span className='m-ck-grow m-ck-note'>{t(option.label)}{unavailable.includes(option.id) ? t('（插件未就绪，保留原配置）') : ''}</span>
          </Label>)}
        </SheetBody>
        <SheetFooter>
          <Button variant='outline' disabled={Boolean(busy)} onClick={onClose}>{t('取消')}</Button>
          <Button disabled={Boolean(busy) || !time || !providers.length} onClick={() => void submit()}>{busy ? t('保存中…') : t('保存')}</Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  )
}
