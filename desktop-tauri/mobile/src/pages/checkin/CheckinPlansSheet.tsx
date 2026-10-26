import { useCallback, useEffect, useRef, useState } from 'react'
import { Button, Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle } from '@ui'
import { Badge } from '../../components/ui'
import { fetchAccountUsage, fetchUsageSnapshot, type UsageRow } from '../../api/gateway'
import { claimPlan, previewPlans, type CaptchaPhase, type Entitlement, type OwnedPlan, type PreviewResult } from '../../api/zcode'
import type { PlanEntry } from '../../api/checkin'
import { claimedPlansToday } from '../../lib/checkin'
import { formatCompact, formatStamp } from '../../lib/format'
import { serverIdentity } from '../../lib/servers'
import { useStore, store } from '../../state/store'
import { t } from '../../i18n'
import { ZcodeCaptchaPanel } from './ZcodeCaptchaPanel'

function stamp(seconds?: number | null): string {
  return seconds ? formatStamp(seconds * 1000) : ''
}

function entitlementText(item: Entitlement): string {
  const period = item.period === 'daily' ? t('每日') : item.period === 'weekly' ? t('每周') : item.period === 'monthly' ? t('每月') : ''
  return [item.showName, item.grantUnits ? `${formatCompact(item.grantUnits)} ${item.unitType || ''}` : '', period].filter(Boolean).join(' · ')
}

function stateView(state?: string): { label: string; tone: 'ok' | 'warn' | 'mute' } {
  if (state === 'active') return { label: t('生效中'), tone: 'ok' }
  if (state === 'pending') return { label: t('待生效'), tone: 'warn' }
  return { label: state === 'expired' ? t('已过期') : t('状态未知'), tone: 'mute' }
}

export function CheckinPlansSheet({ entry, onClose }: { entry: PlanEntry; onClose: () => void }) {
  const { session } = useStore()
  const [preview, setPreview] = useState<PreviewResult | null>(null)
  const [usage, setUsage] = useState<UsageRow | null>(null)
  const [previewError, setPreviewError] = useState('')
  const [usageError, setUsageError] = useState('')
  const [loading, setLoading] = useState(true)
  const [querying, setQuerying] = useState(false)
  const [claiming, setClaiming] = useState('')
  const [claimPhase, setClaimPhase] = useState<CaptchaPhase | ''>('')
  const [verifyingChannel, setVerifyingChannel] = useState(false)
  const [claimedNow, setClaimedNow] = useState<string[]>([])
  const mounted = useRef(true)
  const claimController = useRef<AbortController | null>(null)
  const identity = serverIdentity()
  const account = session?.accounts?.accounts?.find(item => item.id === entry.id)
  const stored = account?.claimPlans as Record<string, unknown> | undefined
  const claimed = [...new Set([...claimedPlansToday(stored || entry.claimPlans), ...claimedNow])]
  const owned = Array.isArray(usage?.usage?.plans) ? usage.usage.plans as OwnedPlan[] : []
  const unconfirmed = claimed.filter(id => !owned.some(plan => plan.planId === id))

  const loadPreview = useCallback(async () => {
    if (!mounted.current || serverIdentity() !== identity) return
    setLoading(true)
    try {
      const next = await previewPlans(entry.id)
      if (mounted.current && serverIdentity() === identity) { setPreview(next); setPreviewError('') }
    } catch (cause) {
      if (mounted.current && serverIdentity() === identity) setPreviewError(cause instanceof Error ? cause.message : String(cause))
    } finally { if (mounted.current && serverIdentity() === identity) setLoading(false) }
  }, [entry.id, identity])

  const queryUsage = useCallback(async () => {
    if (!mounted.current || serverIdentity() !== identity) return
    setQuerying(true)
    try {
      const next = await fetchAccountUsage(entry.id)
      if (!mounted.current || serverIdentity() !== identity) return
      const row = next.results?.find(item => item.id === entry.id)
      setUsage(row || null)
      setUsageError(row?.error || (row ? '' : t('未返回余额数据')))
    } catch (cause) {
      if (mounted.current && serverIdentity() === identity) setUsageError(cause instanceof Error ? cause.message : String(cause))
    } finally { if (mounted.current && serverIdentity() === identity) setQuerying(false) }
  }, [entry.id, identity])

  useEffect(() => {
    mounted.current = true
    const controller = new AbortController()
    void loadPreview()
    void fetchUsageSnapshot(controller.signal).then(snapshot => {
      if (controller.signal.aborted || !mounted.current || serverIdentity() !== identity) return
      const row = snapshot.results?.find(item => item.id === entry.id)
      if (row) { setUsage(row); setUsageError(row.error || '') }
      else void queryUsage()
    }).catch(() => {
      if (!controller.signal.aborted && mounted.current && serverIdentity() === identity) void queryUsage()
    })
    return () => {
      mounted.current = false
      controller.abort()
      claimController.current?.abort()
    }
  }, [entry.id, identity, loadPreview, queryUsage])

  const claim = async (id: string) => {
    if (claimController.current || verifyingChannel || !mounted.current || serverIdentity() !== identity) return
    const controller = new AbortController()
    claimController.current = controller
    setClaiming(id)
    setClaimPhase('preparing')
    try {
      const result = await claimPlan(entry.id, id, controller.signal, phase => { if (mounted.current) setClaimPhase(phase) })
      if (controller.signal.aborted || !mounted.current || serverIdentity() !== identity) return
      if (result.ok || result.failure === 'already_claimed') {
        const settled = result.planId || id
        setClaimedNow(list => [...new Set([...list, settled])])
        store.toast(result.ok ? t('领取成功：{planId}', { planId: settled }) : result.failureLabel || t('今日已领'))
        void store.refresh(true)
        void queryUsage()
        void loadPreview()
      } else {
        store.toast([result.failureLabel || t('未领取成功'), result.message].filter(Boolean).join('：'), 'err')
      }
    } catch (cause) {
      if (!controller.signal.aborted && mounted.current && serverIdentity() === identity && (cause as Error)?.name !== 'CaptchaCancelledError') {
        store.toast(cause instanceof Error ? cause.message : String(cause), 'err')
      }
    } finally {
      if (claimController.current === controller) claimController.current = null
      if (mounted.current && serverIdentity() === identity) { setClaiming(''); setClaimPhase('') }
    }
  }

  const cancelClaim = () => {
    if (claimPhase !== 'submitting') claimController.current?.abort()
  }

  const close = () => {
    if (claimPhase === 'submitting') return
    cancelClaim()
    onClose()
  }

  return (
    <Sheet open modal={!claiming && !verifyingChannel} onOpenChange={(open, details) => {
      if (open) return
      if (claimPhase === 'submitting' || ((claiming || verifyingChannel) && ['outside-press', 'focus-out'].includes(details.reason))) {
        details.cancel()
        return
      }
      close()
    }}>
      <SheetContent showCloseButton={claimPhase !== 'submitting'}>
        <SheetHeader><SheetTitle>{t('套餐明细 — {name}', { name: entry.name || entry.id })}</SheetTitle></SheetHeader>
        <SheetBody>
          <ZcodeCaptchaPanel accountId={entry.id} disabled={Boolean(claiming)} onBusy={setVerifyingChannel} />
          <div className='m-ck-toolbar'>
            <div className='m-ck-title m-ck-grow'>{t('可领取的套餐')}</div>
            <Button variant='outline' size='sm' disabled={loading || Boolean(claiming)} onClick={() => void loadPreview()}>{t('刷新')}</Button>
          </div>
          {previewError ? <p className='m-ck-error'>{previewError}</p> : null}
          {loading ? <p className='m-ck-note'>{t('正在探测可领取的套餐…')}</p>
            : !preview?.plans?.length ? <p className='m-ck-note'>{t('当前没有可领取的套餐')}</p> : null}
          {preview?.plans?.map(plan => {
            const id = plan.planId || ''
            return <div className='m-ck-account' key={id || plan.name}>
              <div className='m-ck-toolbar'>
                <div className='m-ck-title m-ck-grow'>{plan.name || id}</div>
                {claimed.includes(id) ? <Badge tone='ok'>{t('今日已领')}</Badge> : null}
              </div>
              {plan.endsAt ? <p className='m-ck-note'>{t('到期 {time}', { time: stamp(plan.endsAt) })}</p> : null}
              {plan.entitlements?.map((item, index) => <p key={index} className='m-ck-note'>{entitlementText(item)}</p>)}
              {plan.description ? <p className='m-ck-note'>{plan.description}</p> : null}
              <div className='m-ck-actions'><Button size='sm' disabled={Boolean(claiming) || verifyingChannel || claimed.includes(id) || !id} onClick={() => void claim(id)}>
                {claiming === id ? t('领取中…') : claimed.includes(id) ? t('今日已领') : t('领取')}
              </Button></div>
            </div>
          })}
          {preview?.activationError ? <p className='m-ck-note'>{preview.activationError}</p> : null}
          <p className='m-ck-note'>{t('领取需要时会打开官方人机验证；活动套餐额度与编码套餐额度独立。')}</p>
          <div className='m-ck-toolbar'>
            <div className='m-ck-title m-ck-grow'>{t('我的套餐')}</div>
            <Button variant='outline' size='sm' disabled={querying || Boolean(claiming)} onClick={() => void queryUsage()}>
              {querying ? t('查询中…') : t('查询余额')}
            </Button>
          </div>
          {usageError ? <p className='m-ck-error'>{usageError}</p> : null}
          {owned.map(plan => {
            const state = stateView(plan.state)
            return <div className='m-ck-account' key={plan.planId || plan.name}>
              <div className='m-ck-toolbar'><span className='m-ck-title m-ck-grow'>{plan.name || plan.planId}</span><Badge tone={state.tone}>{state.label}</Badge></div>
              {plan.pendingUntil ? <p className='m-ck-note'>{t('待生效 {time}', { time: stamp(plan.pendingUntil) })}</p> : null}
              {plan.endsAt ? <p className='m-ck-note'>{t('到期 {time}', { time: stamp(plan.endsAt) })}</p> : null}
              {plan.entitlements?.map((item, index) => <p className='m-ck-note' key={index}>{entitlementText(item)}</p>)}
            </div>
          })}
          {!owned.length ? <p className='m-ck-note'>{querying ? t('查询中…') : t('当前余额读数里没有套餐清单')}</p> : null}
          {unconfirmed.length ? <p className='m-ck-note'>{t('今天已领取但读数尚未同步：{plans}', { plans: unconfirmed.join('、') })}</p> : null}
        </SheetBody>
        <SheetFooter>
          {claiming ? <Button variant='outline' disabled={claimPhase === 'submitting'} onClick={cancelClaim}>{t('取消验证')}</Button> : null}
          <Button variant='outline' disabled={claimPhase === 'submitting'} onClick={close}>{t('关闭')}</Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  )
}
