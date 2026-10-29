import * as React from 'react'
import {
  Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Input, Label, Progress, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from '@ui'
import { t } from '../i18n'
import { absoluteKernelPath, kernelFileAccept, kernelFileLimit, readKernelFile, type KernelImportInput } from '../../../ui-shared/kernel-import'
import { errorMessage, getSnapshot, importKernel, kernel, loadSection, subscribe, wb } from './proxies-sub-state'
import { openExternal } from './update-shared'

export function KernelImportDialog({ onClose }: { onClose: () => void }) {
  const snapshot = React.useSyncExternalStore(subscribe, getSnapshot)
  const status = kernel()
  const [source, setSource] = React.useState('upload')
  const [file, setFile] = React.useState<File | null>(null)
  const [path, setPath] = React.useState('')
  const [dir, setDir] = React.useState(status?.kernelDir || '')
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [phaseText, setPhaseText] = React.useState('')
  const fileInput = React.useRef<HTMLInputElement>(null)
  const locked = React.useRef(false)
  const active = status?.import?.active === true
  const blocked = busy || active || snapshot.pending || status?.installBusy === true
  const limit = kernelFileLimit(status)
  const options = [{ value: 'upload', label: t('上传本地文件') }, { value: 'path', label: t('网关机器上的文件或目录') }]

  React.useEffect(() => { void loadSection(true) }, [])

  async function chooseDir() {
    try {
      const picked = await wb().workbuddyDesktop?.pickFolder?.(dir.trim() || status?.kernelDirDefault || null)
      if (!picked || picked.unsupported) { setError(t('当前环境不支持选择文件夹，直接填写绝对路径即可')); return }
      if (!picked.canceled && picked.path) setDir(picked.path)
    } catch (cause) { setError(errorMessage(cause)) }
  }

  function selectFile(selected: File | undefined) {
    if (!selected) return
    if (selected.size === 0 || selected.size > limit) {
      setFile(null)
      setError(t('内核文件必须非空且不超过 {max} MiB', { max: Math.floor(limit / 1024 / 1024) }))
      return
    }
    setFile(selected)
    setError('')
  }

  async function install() {
    if (locked.current || blocked) return
    const target = dir.trim()
    if (target && !absoluteKernelPath(target)) { setError(t('存放位置要填绝对路径，例如 D:\\proxy-kernel')); return }
    if (source === 'upload' && !file) { setError(t('请选择要导入的内核文件')); return }
    if (source === 'path' && !absoluteKernelPath(path.trim())) { setError(t('请填写网关机器上的绝对文件或目录路径')); return }
    locked.current = true
    setBusy(true)
    setError('')
    try {
      if (status?.installed) {
        const confirm = window.wbConfirm?.ask
        const text = t('将替换现有代理内核，运行中的连接会短暂中断。订阅、账号引用与端口映射会保留。是否继续？')
        const accepted = confirm ? await confirm({ title: t('替换代理内核'), html: text, okText: t('导入并替换'), okClass: 'danger' }) : window.confirm(text)
        if (!accepted) return
      }
      setPhaseText(t('正在读取并上传文件…'))
      const input: KernelImportInput = { dir: target, replace: status?.installed === true }
      if (source === 'upload' && file) input.fileB64 = await readKernelFile(file)
      else input.path = path.trim()
      await importKernel(input)
      onClose()
    } catch (cause) { setError(errorMessage(cause)) }
    finally { locked.current = false; setBusy(false); setPhaseText('') }
  }

  const phase = status?.import?.state === 'checking' ? t('正在校验内核…')
    : status?.import?.state === 'installing' ? t('正在安装内核…')
    : status?.import?.state === 'starting' ? t('内核正在启动…')
    : t('内核导入任务排队中…')

  return <Dialog open onOpenChange={(open, details) => {
    if (open) return
    if (busy) { details.cancel(); return }
    onClose()
  }}>
    <DialogContent className='w-[min(600px,calc(100vw-48px))]' showCloseButton={!busy}>
      <DialogHeader><DialogTitle>{t('手动导入内核')}</DialogTitle></DialogHeader>
      <DialogBody className='flex flex-col gap-4'>
        <div className='rounded-md border border-border bg-surface-2 p-3 text-xs leading-relaxed'>
          <p>{t('请下载与网关平台一致的官方内核，不是浏览器所在设备的平台。')}</p>
          <p className='mt-2 font-mono break-all'>{status?.platform || '—'} / {status?.arch || '—'} · {status?.version || '—'}</p>
          <p className='mt-1 font-mono break-all select-text'>{status?.asset || '—'}</p>
          <div className='mt-2 flex flex-wrap gap-2'>
            <Button size='sm' variant='outline' disabled={!status?.downloadUrl} onClick={() => void openExternal(status?.downloadUrl || '')}>{t('手动下载官方内核')}</Button>
            <Button size='sm' variant='ghost' disabled={!status?.downloadUrl} data-copy={status?.downloadUrl || ''}>{t('复制下载链接')}</Button>
          </div>
          <p className='mt-2'>{t('支持官方压缩包和解压后的内核二进制；只接受当前版本，SHA-256 校验不通过时不会替换现有内核。')}</p>
          {status?.sha256 ? <p className='mt-2 break-all font-mono select-text'>SHA-256: {status.sha256}</p> : null}
        </div>
        <div className='flex flex-col gap-1.5'>
          <Label htmlFor='kernel-import-source'>{t('导入来源')}</Label>
          <Select value={source} disabled={blocked} onValueChange={value => { setSource(String(value)); setError('') }}>
            <SelectTrigger id='kernel-import-source'><SelectValue>{options.find(option => option.value === source)?.label}</SelectValue></SelectTrigger>
            <SelectContent>{options.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
          </Select>
        </div>
        {source === 'upload' ? <div className='flex flex-col gap-2'>
          <Input ref={fileInput} type='file' accept={kernelFileAccept(status)} className='hidden' disabled={blocked}
            onChange={event => selectFile(event.currentTarget.files?.[0])} />
          <Button variant='outline' disabled={blocked} onClick={() => { if (fileInput.current) { fileInput.current.value = ''; fileInput.current.click() } }}>{t('选择内核文件…')}</Button>
          <p className='break-all text-xs text-subtle'>{file ? `${file.name} · ${(file.size / 1024 / 1024).toFixed(1)} MiB` : t('未选择文件')}</p>
        </div> : <div className='flex flex-col gap-1.5'>
          <Label htmlFor='kernel-import-path'>{t('文件或目录路径')}</Label>
          <Input id='kernel-import-path' value={path} disabled={blocked} autoComplete='off' spellCheck={false} className='font-mono text-xs'
            placeholder={status?.kernelDir || ''} onChange={event => setPath(event.currentTarget.value)} />
          <p className='text-xs leading-relaxed text-subtle'>{t('先把下载的内核放到网关机器的目录，再填写文件或目录的绝对路径。目录内会查找官方包或 mihomo 二进制；远程面板不能读取浏览器电脑的路径。')}</p>
        </div>}
        <div className='flex flex-col gap-1.5'>
          <Label htmlFor='kernel-import-dir'>{t('存放位置')}</Label>
          <div className='flex items-center gap-2'>
            <Input id='kernel-import-dir' value={dir} disabled={blocked} placeholder={status?.kernelDirDefault || ''} className='min-w-0 flex-1 font-mono text-xs'
              autoComplete='off' spellCheck={false} onChange={event => setDir(event.currentTarget.value)} />
            <Button size='sm' variant='outline' disabled={blocked} onClick={() => void chooseDir()}>{t('选择目录…')}</Button>
            <Button size='sm' variant='ghost' disabled={blocked} onClick={() => setDir(status?.kernelDirDefault || '')}>{t('恢复默认')}</Button>
          </div>
          <p className='text-xs leading-relaxed text-subtle'>{t('这是网关机器上的安装目录，留空使用默认位置。导入会复制文件并建立版本目录，不删除原下载文件。')}</p>
        </div>
        {Boolean(busy && phaseText) || active ? <div className='flex flex-col gap-2'><Progress value={null} /><p className='text-xs'>{active ? phase : phaseText}</p></div> : null}
        {error ? <Alert variant='destructive'>{error}</Alert> : null}
      </DialogBody>
      <DialogFooter>
        <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={blocked || !status?.asset} onClick={() => void install()}>{busy && phaseText ? t('正在导入…') : active ? t('正在导入…') : status?.installed ? t('导入并替换') : t('导入内核')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
