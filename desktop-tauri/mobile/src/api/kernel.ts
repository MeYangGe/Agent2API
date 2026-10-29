import { apiCall } from './client'
import type { KernelImportInput, KernelImportStatus } from '../../../ui-shared/kernel-import'

export type KernelStatus = KernelImportStatus & {
  download?: { state: string; error?: string | null }
}

export function getKernelStatus(): Promise<KernelStatus> {
  return apiCall('/api/subscriptions/kernel')
}

export function startKernelImport(input: KernelImportInput): Promise<KernelStatus> {
  return apiCall('/api/subscriptions/kernel/import', { method: 'POST', body: input })
}
