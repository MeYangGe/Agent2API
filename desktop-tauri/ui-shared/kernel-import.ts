export type KernelImportProgress = {
  id: string
  active: boolean
  state: 'idle' | 'queued' | 'checking' | 'installing' | 'starting' | 'done' | 'error'
  error?: string | null
  startError?: string | null
  saved?: boolean
}

export type KernelImportInput = {
  dir?: string
  fileB64?: string
  path?: string
  replace?: boolean
}

export type KernelImportStatus = {
  installed: boolean
  version: string
  installedVersion?: string | null
  platform?: string
  arch?: string
  asset?: string | null
  binaryName?: string
  downloadUrl?: string | null
  releaseUrl?: string
  sha256?: string | null
  maxImportBytes?: number
  kernelDir: string
  kernelDirDefault?: string
  running: boolean
  halted?: boolean
  lastError?: string | null
  installBusy?: boolean
  import?: KernelImportProgress
}

export function absoluteKernelPath(value: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(value) || value.startsWith('\\\\') || value.startsWith('/')
}

export function kernelFileLimit(status: KernelImportStatus | null | undefined): number {
  const limit = Number(status?.maxImportBytes)
  return Number.isFinite(limit) && limit > 0 ? limit : 128 * 1024 * 1024
}

export function kernelFileAccept(status: KernelImportStatus | null | undefined): string {
  return status?.platform === 'windows' ? '.zip,.exe' : ''
}

export function readKernelFile(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      const data = String(reader.result || '')
      const start = data.indexOf('base64,')
      if (start < 0) { reject(new Error('文件读取失败')); return }
      resolve(data.slice(start + 7))
    }
    reader.onerror = () => reject(new Error('文件读取失败'))
    reader.onabort = () => reject(new DOMException('文件读取已取消', 'AbortError'))
    reader.readAsDataURL(file)
  })
}
