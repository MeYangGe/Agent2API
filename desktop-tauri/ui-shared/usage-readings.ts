type Translate = (key: string, params?: Record<string, string | number>) => string

type UsageObject = Record<string, unknown>

export type SnapshotReading = {
  name: string
  view: string
  tone: 'ok' | 'muted'
  daily: boolean
}

function object(value: unknown): UsageObject | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as UsageObject : null
}

function numberOrNull(value: unknown): number | null {
  if (typeof value !== 'number' && typeof value !== 'string') return null
  if (typeof value === 'string' && !value.trim()) return null
  const number = Number(value)
  return Number.isFinite(number) ? number : null
}

/** 钱包当前余额不是套餐发放上限；旧版魔粒钱包没有 snapshot 标记。 */
export function isSnapshotWallet(value: unknown): boolean {
  const wallet = object(value)
  return wallet?.snapshot === true || wallet?.type === 'magicubes'
}

/** 旧版魔粒明细把钱包余额和已用次数都写成了 remain，兼容时只转换这一种钱包。 */
export function creditPacksOf(value: unknown): UsageObject[] {
  const usage = object(value)
  if (!usage) return []
  const packs = Array.isArray(usage.credits) ? usage.credits.filter(item => object(item)) as UsageObject[] : []
  const legacy = Array.isArray(usage.wallets) && usage.wallets.some(wallet => object(wallet)?.type === 'magicubes')
  if (!legacy) return packs
  return packs.map(pack => {
    if (pack.snapshot === true || pack.record === true) return pack
    const { remain, ...rest } = pack
    switch (pack.name) {
      case '可用魔粒':
        return { ...rest, key: 'available', name: '可用余额', balance: remain, unit: '魔粒', snapshot: true }
      case '冻结魔粒':
        return { ...rest, key: 'frozen', name: '冻结余额', balance: remain, unit: '魔粒', snapshot: true }
      case '今日已用（次）':
        return { ...rest, key: 'used-today', name: '今日已用', used: remain, unit: '次', daily: true, snapshot: true }
      default:
        return pack
    }
  })
}

export function usageUnitLabel(unit: string, t: Translate): string {
  switch (unit) {
    case '积分': return t('积分')
    case '额度': return t('额度')
    case '魔粒': return t('魔粒')
    case '次': return t('次')
    default: return unit
  }
}

/** snapshot 读数不表示一份积分包：不计算耗尽、到期或剩余比例。 */
export function snapshotReadingOf(pack: UsageObject, t: Translate, format: (value: number) => string): SnapshotReading | null {
  if (pack.snapshot !== true) return null
  const used = numberOrNull(pack.used)
  const value = used ?? numberOrNull(pack.balance) ?? numberOrNull(pack.remain)
  const unit = String(pack.unit || '').trim()
  const rawName = String(pack.name || '').trim()
  const name = rawName === '可用余额' ? t('可用余额')
    : rawName === '冻结余额' ? t('冻结余额')
      : rawName === '今日已用' ? t('今日已用')
        : rawName || t('明细')
  const view = value === null ? '—'
    : unit === '次' ? t('{count} 次', { count: format(value) })
      : unit ? `${format(value)} ${usageUnitLabel(unit, t)}` : format(value)
  return {
    name,
    view,
    tone: used !== null || pack.key === 'frozen' || value === null || value <= 0 ? 'muted' : 'ok',
    daily: pack.daily === true,
  }
}

export function snapshotReadingsOf(value: unknown, t: Translate, format: (value: number) => string): SnapshotReading[] {
  return creditPacksOf(value)
    .map(pack => snapshotReadingOf(pack, t, format))
    .filter((reading): reading is SnapshotReading => reading !== null)
}
