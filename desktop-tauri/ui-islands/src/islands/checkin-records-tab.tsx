import * as React from 'react'
import { Badge, Button, Input, SegmentedControl, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Card, CardHeader, Table, TableHeader, TableRow, TableHead, TableBody, TableCell } from '@ui'
import { clearRecords, loadRecords, patch, store } from './checkin-store'
import {
  OUTCOME_VIEW, SOURCE_LABEL, formatShort, formatTime,
  type RecordRow,
} from './checkin-types'
import { confirmDialog } from '@ui'
import { Empty } from '@ui'

/**
 * 「签到记录」标签页：每账号一行的明细台账。
 *
 * ── 这页回答什么 ────────────────────────────────────────────
 * 「到底领到了没、失败的话错在哪」。每执行一次签到（定时 / 启动补签 / 手动批量 /
 * 单账号 / 国际版活动），每个账号落一行。判据在后端唯一（复用签到执行体的同一对
 * 函数），这里只按 `outcome` 上色，不自己再判一遍。
 *
 * ── 分页 ────────────────────────────────────────────────────
 * 服务端真分页（`limit` / `offset` + 同条件 `total`）。筛选变化时 offset 归零。
 */

/** 时间档位 */
const RANGES = [
  { value: 'all', label: '全部' },
  { value: 'today', label: '今天' },
  { value: '7', label: '7 天' },
  { value: '30', label: '30 天' },
  { value: 'month', label: '本月' },
]

/** 结果档位（后端 `outcome` 的取值域） */
const OUTCOME_OPTIONS = [
  { value: '', label: '全部结果' },
  { value: 'success', label: '成功领取' },
  { value: 'already', label: '今日已领' },
  { value: 'failed', label: '失败' },
]

const PAGE_SIZE = 50

/** 时间档位 → 毫秒下界（含今天在内的 N 个自然日；`all` = 不限） */
function rangeStart(value: string): number | null {
  const now = new Date()
  const midnight = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
  switch (value) {
    case 'today': return midnight(now)
    case '7': return midnight(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 6))
    case '30': return midnight(new Date(now.getFullYear(), now.getMonth(), now.getDate() - 29))
    case 'month': return midnight(new Date(now.getFullYear(), now.getMonth(), 1))
    default: return null
  }
}

/** 说明列：上游文案优先，失败时用错误原文 */
function detailText(row: RecordRow): string {
  if (row.message) return row.message
  if (row.error) return row.error
  if (row.keptActive) return '保活成功'
  return '—'
}

export function RecordsTab(): React.ReactElement {
  const query = store.recordsQuery
  const [range, setRange] = React.useState('all')
  const [keyword, setKeyword] = React.useState(query.keyword)

  const total = store.recordsTotal
  const pageCount = Math.max(1, Math.ceil(total / PAGE_SIZE))
  const page = Math.min(Math.floor(query.offset / PAGE_SIZE) + 1, pageCount)

  const apply = (next: Partial<typeof query>) => {
    patch({ recordsQuery: { ...query, ...next, offset: 0 } })
    void loadRecords()
  }

  const changeRange = (value: string) => {
    setRange(value)
    const start = rangeStart(value)
    patch({ recordsQuery: { ...query, start: start ?? undefined, offset: 0 } })
    void loadRecords()
  }

  const goPage = (target: number) => {
    const clamped = Math.max(1, Math.min(target, pageCount))
    patch({ recordsQuery: { ...query, offset: (clamped - 1) * PAGE_SIZE } })
    void loadRecords()
  }

  return (
    <div className="ck-main">
      <Card render={<section />} className="panel">
        <CardHeader className="panel-head">
          <h2>签到记录</h2>
          <span className="panel-sub">每执行一次签到 · 每个账号一条明细 · 结果与收益以后端落账为准</span>
          <Button
            size="sm"
            variant="destructive"
            className="ml-auto flex-none"
            onClick={async () => {
              if (!await confirmDialog('确定清空全部签到记录？清空后无法恢复。')) return
              void clearRecords(true)
            }}
          >
            清空全部
          </Button>
        </CardHeader>

        <div className="ck-filters">
          <SegmentedControl
            options={RANGES}
            value={range}
            onValueChange={changeRange}
            aria-label="时间范围"
          />
          <Select value={query.outcome} onValueChange={value => apply({ outcome: String(value) })}>
            <SelectTrigger aria-label='签到结果'>
              <SelectValue>{OUTCOME_OPTIONS.find(option => option.value === query.outcome)?.label || '全部结果'}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              {OUTCOME_OPTIONS.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
            </SelectContent>
          </Select>
          <Input
            type="search"
            className="ck-input-search"
            placeholder="搜索账号 / 说明"
            value={keyword}
            onChange={event => setKeyword(event.currentTarget.value)}
            onKeyDown={event => {
              // 回车才查：每敲一个字都打一次网关没必要
              if (event.key === 'Enter') apply({ keyword: keyword.trim() })
            }}
            onBlur={() => apply({ keyword: keyword.trim() })}
          />
          <span className="ck-filters-count">共 {total} 条</span>
          <Button size="sm" variant="ghost" onClick={() => void loadRecords()}>刷新</Button>
        </div>

        {store.recordsError ? (
          <div className="ck-fold-note">读取失败：{store.recordsError}</div>
        ) : null}

        {store.recordsLoading && store.records.length === 0 ? (
          <Empty className="ck-empty">正在读取签到记录…</Empty>
        ) : store.records.length === 0 ? (
          <Empty className="ck-empty">这段时间里还没有签到记录。</Empty>
        ) : (
          <div className="ck-records-scroll">
            <Table container={false} className="ck-table ck-records-table">
              <colgroup>
                <col className="ck-col-time" />
                <col className="ck-col-account" />
                <col className="ck-col-provider" />
                <col className="ck-col-source" />
                <col className="ck-col-outcome" />
                <col className="ck-col-detail" />
              </colgroup>
              <TableHeader>
                <TableRow>
                  <TableHead>时间</TableHead>
                  <TableHead>账号</TableHead>
                  <TableHead>提供商</TableHead>
                  <TableHead>来源</TableHead>
                  <TableHead>结果</TableHead>
                  <TableHead>说明</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {store.records.map((row, index) => {
                  const view = OUTCOME_VIEW[row.outcome ?? '']
                  return (
                    <TableRow key={`${row.ts}-${row.accountId ?? index}`} className={row.outcome === 'failed' ? 'ck-row-fail' : undefined}>
                      <TableCell className="ck-col-time">
                        <span>{formatShort(row.ts)}</span>
                        {row.date ? <span className="ck-record-date">{row.date}</span> : null}
                      </TableCell>
                      <TableCell>{row.accountName || row.accountId || '—'}</TableCell>
                      {/* 后端读侧已把 provider id 换成展示名（`qoder-intl`
                          → 「Qoder 国际版」），这里直接显示 */}
                      <TableCell className="ck-record-source">{row.provider || '—'}</TableCell>
                      <TableCell>
                        <span>{SOURCE_LABEL[row.source ?? ''] ?? row.source ?? '—'}</span>
                        {row.reason ? <span className="ck-record-reason">{row.reason}</span> : null}
                      </TableCell>
                      <TableCell>
                        {view
                          ? <Badge variant={view.variant} shape="tag">{view.label}</Badge>
                          : <Badge variant="outline" shape="tag">{row.outcome || '—'}</Badge>}
                        {row.keptActive ? <Badge variant="outline" shape="tag">保活</Badge> : null}
                      </TableCell>
                      <TableCell className={row.error ? 'ck-record-detail-fail' : undefined}>
                        {detailText(row)}
                      </TableCell>
                    </TableRow>
                  )
                })}
              </TableBody>
            </Table>
          </div>
        )}

        {total > PAGE_SIZE ? (
          <div className="ck-pager">
            <Button size="sm" variant="outline" disabled={page <= 1} onClick={() => goPage(1)}>首页</Button>
            <Button size="sm" variant="outline" disabled={page <= 1} onClick={() => goPage(page - 1)}>上一页</Button>
            <span className="ck-pager-label">{page} / {pageCount}</span>
            <Button size="sm" variant="outline" disabled={page >= pageCount} onClick={() => goPage(page + 1)}>下一页</Button>
            <Button size="sm" variant="outline" disabled={page >= pageCount} onClick={() => goPage(pageCount)}>末页</Button>
          </div>
        ) : null}
      </Card>
    </div>
  )
}

/** 供外部读的格式化（`checkin-page` 的顶栏用得上时间戳） */
export { formatTime }
