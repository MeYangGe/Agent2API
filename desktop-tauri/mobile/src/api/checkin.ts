import { apiCall } from './client'
import type {
  AutoState, CheckinCenter, CheckinResult, CheckinResultRow, KeepaliveState, OnboardingTask,
  RecordsPage, TravelRunResult, TravelState,
} from '../../../ui-shared/checkin'

export type {
  AutoState, CheckinAccount, CheckinCenter, CheckinResult, CheckinSection, GrowthEntry,
  KeepaliveState, OnboardingEntry, OnboardingTask, PlanEntry, ProviderGroup,
  RecordRow, RecordsPage, ScheduleEntry, TravelRunResult, TravelState,
} from '../../../ui-shared/checkin'
export { CHECKIN_SECTION_OPTIONS, modesOf } from '../../../ui-shared/checkin'

export type CheckinRun = CheckinResult
export type ActivityResult = CheckinResultRow

export type SchedulePatch = {
  id?: string
  label?: string
  enabled: boolean
  time: string
  providers: string[]
}

export type RecordFilters = {
  outcome?: string
  provider?: string
  keyword?: string
  start?: number
  end?: number
}

function queryString(params: Record<string, unknown>): string {
  const query = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== '') query.set(key, String(value))
  }
  const text = query.toString()
  return text ? `?${text}` : ''
}

const accountPath = (id: string) => `/api/accounts/${encodeURIComponent(id)}`

export function getCheckinCenter(signal?: AbortSignal): Promise<CheckinCenter> {
  return apiCall('/api/checkin-center', { signal })
}

export function runCheckin(id?: string): Promise<CheckinRun> {
  return apiCall('/api/accounts/checkin', { method: 'POST', body: id ? { id } : {} })
}

export function runActivity(id: string, mode: string): Promise<ActivityResult> {
  return apiCall('/api/checkin-activity', { method: 'POST', body: { id, mode } })
}

export function saveAutoCheckin(enabled: boolean): Promise<AutoState> {
  return apiCall('/api/auto-checkin', { method: 'POST', body: { enabled } })
}

export function saveCheckinSchedule(body: SchedulePatch): Promise<AutoState> {
  return apiCall('/api/checkin-schedules', { method: 'POST', body })
}

export function deleteCheckinSchedule(id: string): Promise<unknown> {
  return apiCall(`/api/checkin-schedules/${encodeURIComponent(id)}/delete`, { method: 'POST' })
}

export function runCheckinSchedule(id: string): Promise<CheckinRun & AutoState> {
  return apiCall(`/api/checkin-schedules/${encodeURIComponent(id)}/run`, { method: 'POST' })
}

export function saveKeepalive(models: string[], apiKeyId?: string | null): Promise<KeepaliveState> {
  return apiCall('/api/checkin-keepalive', { method: 'POST', body: { models, apiKeyId } })
}

export function saveTravel(body: { enabled?: boolean; times?: string[] }): Promise<TravelState> {
  return apiCall('/api/travel-schedule', { method: 'POST', body })
}

export function runTravel(): Promise<TravelRunResult> {
  return apiCall('/api/travel-schedule/run', { method: 'POST' })
}

export function getCheckinRecords(
  filters: RecordFilters & { offset: number; limit: number },
  signal?: AbortSignal,
): Promise<RecordsPage> {
  return apiCall(`/api/checkin-records${queryString(filters)}`, { signal })
}

export function clearCheckinRecords(filters: RecordFilters, all = false): Promise<{ removed: number }> {
  return apiCall(`/api/checkin-records${queryString(all ? { all: 1 } : filters)}`, { method: 'DELETE' })
}

export type OnboardingResult = {
  tasks?: OnboardingTask[]
  earned?: number
  total?: number
  unclaimed?: number
  settled?: boolean
  settledAt?: number | null
  claimed?: number
  failed?: number
  claimedPoints?: number
  message?: string
}

export function getOnboarding(id: string, refresh = false): Promise<OnboardingResult> {
  return apiCall(`${accountPath(id)}/onboarding${refresh ? '?refresh=1' : ''}`)
}

export function claimOnboarding(id: string): Promise<OnboardingResult> {
  return apiCall(`${accountPath(id)}/onboarding/claim`, { method: 'POST' })
}

export type GrowthTask = {
  code: string
  title: string
  description?: string
  status?: string
  current?: number
  target?: number
  locked?: boolean
  supported?: boolean
  unforgeable?: boolean
}

export type GrowthResult = {
  energy?: number | null
  streak?: number | null
  tasks?: { tasks?: GrowthTask[]; error?: string }
  travel?: Record<string, unknown> | null
  school?: Record<string, unknown> | null
}

export type GrowthRun = {
  code?: string
  outcome?: string
  message?: string
  error?: string
  credit?: number
}

export function getGrowth(id: string): Promise<GrowthResult> {
  return apiCall(`${accountPath(id)}/growth`)
}

export function runGrowthTask(id: string, code: string, mode: 'full' | 'claim'): Promise<GrowthRun> {
  return apiCall(`${accountPath(id)}/growth/task`, { method: 'POST', body: { code, mode } })
}

export function runGrowthTravel(id: string): Promise<GrowthRun> {
  return apiCall(`${accountPath(id)}/growth/travel`, { method: 'POST' })
}

export function runGrowthSchool(id: string, lottery: boolean): Promise<GrowthRun> {
  return apiCall(`${accountPath(id)}/growth/school`, { method: 'POST', body: { lottery } })
}

export type UmidState = { available?: boolean; installing?: boolean; installSupported?: boolean; source?: string }

export function getUmid(): Promise<UmidState> {
  return apiCall('/api/qoder-umid')
}

export function installUmid(): Promise<UmidState> {
  return apiCall('/api/qoder-umid/install', { method: 'POST' })
}
