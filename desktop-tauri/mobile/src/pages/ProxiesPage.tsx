import { useState } from 'react'
import { PageIntro, IconButton, Badge, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { GlyphLogo } from '../components/ProviderLogo'
import { Icon } from '../components/Icon'
import { PullIndicator, usePullToRefresh } from '../components/PullToRefresh'
import { store } from '../state/store'
import {
  fetchProxyPool,
  setProxyEnabled,
  syncClashProxies,
  testProxy,
  type PoolItem,
  type PoolPayload,
} from '../api/gateway'
import { formatRelative } from '../lib/format'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'
import { hasProxyUsage, proxyServiceLabels } from '../../../ui-shared/proxy-usage'
import { Card, Button } from '@ui'
import { Switch as UiSwitch } from '@ui'
import { getKernelStatus, type KernelStatus } from '../api/kernel'
import { KernelImportSheet } from './KernelImportSheet'
import { serverIdentity } from '../lib/servers'

/**
 * 网络代理。
 *
 * 每个出口一张卡：地址、上次测试结果、被几个账号引用、开关。
 * 「引用数」是删除前最需要知道的事 —— 删掉一个还有账号挂着的出口，
 * 那些账号会静默回退直连。
 *
 * 「测试连通性」是行内按钮，结果就地显示在卡里，不弹窗打断。
 */
export function ProxiesPage() {
  const [data, setData] = useState<PoolPayload | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')
  const [tested, setTested] = useState<Record<string, { ok: boolean; text: string }>>({})
  const [kernel, setKernel] = useState<KernelStatus | null>(null)
  const [importOpen, setImportOpen] = useState(false)
  const [kernelError, setKernelError] = useState('')
  const identity = serverIdentity()

  const load = async () => {
    try {
      const next = await fetchProxyPool()
      if (serverIdentity() !== identity) return
      setData(next)
      setError('')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    }
  }

  const loadKernel = async () => {
    try {
      const next = await getKernelStatus()
      if (serverIdentity() !== identity) return
      setKernel(next)
      setKernelError('')
    } catch (cause) {
      if (serverIdentity() === identity) setKernelError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  usePolling(load, 120_000)
  usePolling(loadKernel, kernel?.installBusy ? 1500 : 15_000)

  const refresh = async () => { await Promise.all([load(), loadKernel()]) }
  const pull = usePullToRefresh({ onRefresh: refresh })

  const items = data?.items || []
  const enabled = items.filter(item => item.enabled).length
  const clash = data?.clash

  const toggle = async (item: PoolItem, next: boolean) => {
    if (!item.id) return
    setBusy(item.id)
    try {
      const payload = await setProxyEnabled(item.id, next)
      setData(payload)
      store.toast(next ? t('已启用「{name}」', { name: item.name || item.id }) : t('已禁用「{name}」', { name: item.name || item.id }))
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  const test = async (item: PoolItem) => {
    if (!item.id) return
    setBusy(item.id)
    try {
      const result = await testProxy(item.id)
      const ok = result.ok !== false && !result.error
      setTested(prev => ({
        ...prev,
        [item.id as string]: {
          ok,
          text: ok ? (result.ip ? t('出口 IP {ip}', { ip: result.ip }) : t('连接成功')) : String(result.error || t('连接失败')),
        },
      }))
      store.toast(ok ? t('出口可用') : t('连接失败'), ok ? 'ok' : 'err')
      await load()
    } catch (failure) {
      const message = failure instanceof Error ? failure.message : String(failure)
      setTested(prev => ({ ...prev, [item.id as string]: { ok: false, text: message } }))
      store.toast(message, 'err')
    } finally {
      setBusy('')
    }
  }

  const sync = async () => {
    setBusy('sync')
    try {
      const result = await syncClashProxies()
      setData(result)
      if (result.syncError) store.toast(t('Clash 同步失败：{msg}', { msg: result.syncError }), 'err')
      else store.toast(t('已同步 {n} 项变更', { n: result.changes || 0 }))
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  return (
    <div className='m-page' ref={pull.bindRef}>
      <PullIndicator pull={pull.pull} refreshing={pull.refreshing} />
      <PageIntro actions={<IconButton name='refresh' label={t('刷新')} onClick={() => void refresh()} />}
      />

      <div className='m-section' style={{ marginTop: 0 }}>
        <Card className='m-card'>
          <Button size='default' variant='ghost' type='button' className='m-row h-auto min-h-[52px] w-full justify-start rounded-none border-0 px-3.5 py-3 text-left' onClick={() => void sync()} disabled={busy === 'sync'}>
            <GlyphLogo name='refresh' />
            <div className='m-grow'>
              <div className='m-t1'>{busy === 'sync' ? t('同步中…') : t('同步 Clash Verge 出口')}</div>
              <div className='m-t2'>
                {clash?.available
                  ? t('镜像当前出口集合，名称与端口跟随 Clash')
                  : clash?.error || t('本机没有读到 Clash Verge 配置')}
              </div>
            </div>
            <span className='m-chev'>
              <Icon name='chev' />
            </span>
          </Button>
        </Card>
      </div>

      {kernel ? <div className='m-section' style={{ marginTop: 0 }}>
        <Card className='m-card m-card-pad'>
          <div className='m-t1'>{t('代理内核')} · {kernel.running ? t('内核运行中') : kernel.installed ? t('内核已停止') : t('内核未安装')}</div>
          <p className='m-t2 m-mono break-all'>{kernel.platform} / {kernel.arch} · {kernel.installedVersion || kernel.version}</p>
          <p className='m-t2 break-all'>{kernel.kernelDir}</p>
          {kernel.import?.active ? <p className='m-t2'>{t('正在导入…')}</p> : null}
          {kernel.import?.error ? <p className='m-t2 text-destructive'>{kernel.import.error}</p> : null}
          <Button className='mt-3' variant='outline' disabled={kernel.installBusy && !kernel.import?.active} onClick={() => setImportOpen(true)}>{t('手动导入内核')}</Button>
        </Card>
      </div> : null}
      {kernelError ? <ErrorNote text={kernelError} /> : null}
      {importOpen && kernel ? <KernelImportSheet status={kernel} onClose={() => setImportOpen(false)} onChanged={refresh} /> : null}

      {error ? <ErrorNote text={error} /> : null}
      {!data ? <SkeletonList rows={3} height={104} /> : null}

      {data && !items.length ? (
        <div className='m-section'>
          <Card className='m-card'>
            <Empty text={t('还没有出口。在桌面端「网络代理」页新增，或点上面同步 Clash。')} />
          </Card>
        </div>
      ) : null}

      {items.map(item => {
        const id = String(item.id || '')
        const result = tested[id]
        const last = item.lastTest
        const ok = result ? result.ok : last?.success
        const text = result
          ? result.text
          : last?.at
            ? last.success
              ? last.ip
                ? t('出口 IP {ip}', { ip: last.ip })
                : t('连接成功')
              : String(last.error || t('连接失败'))
            : t('未测试')
        return (
          <div key={id} className='m-section' style={{ marginTop: 10 }}>
            <Card className='m-card m-card-pad'>
              <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                <GlyphLogo name='globe' />
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 14, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {item.name || t('未命名')}
                  </div>
                  <div style={{ fontSize: 11.5, color: 'var(--m-text-3)', marginTop: 2, fontFamily: 'var(--m-mono)' }}>
                    {addressOf(item, data?.kernelInstalled)}
                  </div>
                </div>
                <SwitchSmall
                  checked={Boolean(item.enabled)}
                  disabled={busy === id}
                  label={item.name || id}
                  onChange={next => void toggle(item, next)}
                />
              </div>

              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 10,
                  marginTop: 11,
                  paddingTop: 10,
                  borderTop: '1px solid var(--m-line)',
                  fontSize: 11.5,
                  color: 'var(--m-text-3)',
                }}
              >
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 5, minWidth: 0 }}>
                  <i
                    style={{
                      width: 6,
                      height: 6,
                      borderRadius: '50%',
                      flex: 'none',
                      background: ok === undefined ? 'var(--m-text-3)' : ok ? 'var(--m-ok)' : 'var(--m-danger)',
                    }}
                  />
                  <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{text}</span>
                </span>
                {item.usedBy?.length ? <span style={{ flex: 'none' }}>{t('{n} 个账号引用', { n: item.usedBy.length })}</span> : null}
                {proxyServiceLabels(item, t).map(service => <span key={service} style={{ flex: 'none' }}>{service}</span>)}
                {last?.at ? <span style={{ flex: 'none' }}>{formatRelative(last.at)}</span> : null}
                <Button size='touch-sm' variant='outline'
                  type='button'
                  className='m-btn m-sm m-ghost'
                  style={{ marginLeft: 'auto', height: 28, fontSize: 11.5, padding: '0 10px' }}
                  disabled={busy === id}
                  onClick={() => void test(item)}
                >
                  {busy === id ? t('测试中…') : t('测试')}
                </Button>
              </div>

              {item.source === 'subscription' ? (
                <div style={{ marginTop: 8 }}>
                  <Badge tone='brand'>{t('订阅节点')}</Badge>
                </div>
              ) : item.source === 'clash' ? (
                <div style={{ marginTop: 8 }}>
                  <Badge tone='mute'>{t('Clash 镜像')}</Badge>
                </div>
              ) : null}
            </Card>
          </div>
        )
      })}

      <div className='m-section' style={{ paddingBottom: 8 }}>
        <Card className='m-card m-card-pad' style={{ fontSize: 11.5, color: 'var(--m-text-3)', lineHeight: 1.7 }}>
          {t('新增出口、编辑地址与订阅管理在桌面端完成；手机端做启停与连通性测试。')}
        </Card>
      </div>
    </div>
  )
}

function addressOf(item: PoolItem, installed?: boolean): string {
  if (item.address) return String(item.address)
  const protocol = item.protocol || 'http'
  if (item.host && item.port) return `${protocol}://${item.host}:${item.port}`
  // 订阅节点没有地址 —— 按成因分档，与桌面端同一口径。**首要判据是内核**：
  // 没装内核就没有监听器，此时说「未被使用」会把用户引去账号页（做无用功）
  if (item.source === 'subscription') {
    if (installed === false) return t('内核未安装')
    if (item.orphan) return t('订阅已移除')
    if (item.enabled === false) return t('已禁用')
    return hasProxyUsage(item) ? t('端口准备中') : t('未被使用')
  }
  return t('地址不完整')
}

function SwitchSmall({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean
  onChange: (next: boolean) => void
  disabled?: boolean
  label: string
}) {
  return (
    <UiSwitch size='touch' variant='success' checked={checked} aria-label={label} disabled={disabled} onCheckedChange={onChange} />
  )
}
