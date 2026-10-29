import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Input, Label, Progress, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle } from '@ui'
import { t } from '../i18n'
import { store } from '../state/store'
import { serverIdentity } from '../lib/servers'
import { openExternal } from '../lib/community'
import { getKernelStatus, startKernelImport, type KernelStatus } from '../api/kernel'
import { absoluteKernelPath, kernelFileAccept, kernelFileLimit, readKernelFile, type KernelImportInput } from '../../../ui-shared/kernel-import'

export function KernelImportSheet({ status, onClose, onChanged }: { status: KernelStatus; onClose: () => void; onChanged: () => Promise<void> }) {
  const [source, setSource] = useState('upload')
  const [path, setPath] = useState('')
  const [dir, setDir] = useState(status.kernelDir || '')
  const [file, setFile] = useState<File | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [confirm, setConfirm] = useState(false)
  const [phase, setPhase] = useState('')
  const [current, setCurrent] = useState(status)
  const inputRef = useRef<HTMLInputElement>(null)
  const lock = useRef(false)
  const identity = serverIdentity()
  const options = [{ value: 'upload', label: t('上传本地文件') }, { value: 'path', label: t('网关机器上的文件或目录') }]
  const limit = kernelFileLimit(status)
  useEffect(() => { if (!busy) setCurrent(status) }, [status, busy])

  async function install(replace = false) {
    if (lock.current || current.installBusy) return
    if (dir.trim() && !absoluteKernelPath(dir.trim())) { setError(t('请填写网关机器上的绝对文件或目录路径')); return }
    if (source === 'upload' && !file) { setError(t('请选择要导入的内核文件')); return }
    if (source === 'path' && !absoluteKernelPath(path.trim())) { setError(t('请填写网关机器上的绝对文件或目录路径')); return }
    if (current.installed && !replace) { setConfirm(true); return }
    lock.current = true
    setConfirm(false)
    setBusy(true)
    setError('')
    setPhase(t('正在读取并上传文件…'))
    try {
      const input: KernelImportInput = { dir: dir.trim(), replace }
      if (source === 'upload' && file) input.fileB64 = await readKernelFile(file)
      else input.path = path.trim()
      if (serverIdentity() !== identity) return
      const first = await startKernelImport(input)
      if (serverIdentity() !== identity) return
      const id = first.import?.id
      if (!id) throw new Error(t('网关未返回内核导入任务，请刷新状态后确认'))
      for (let round = 0; round < 300; round += 1) {
        const next = round === 0 ? first : await getKernelStatus()
        if (serverIdentity() !== identity) return
        if (!next.import || next.import.id !== id) throw new Error(t('内核导入状态已变化，请刷新后确认'))
        setCurrent(next)
        setPhase(next.import.state === 'checking' ? t('正在校验内核…') : next.import.state === 'installing' ? t('正在安装内核…')
          : next.import.state === 'starting' ? t('内核正在启动…') : t('内核导入任务排队中…'))
        if (!next.import.active) {
          if (next.import.state !== 'done') throw new Error(next.import.error || t('内核导入失败'))
          if (next.import.saved === false) store.toast(t('内核已导入，但存放目录未能保存，重启网关后会恢复原位置'), 'err')
          if (next.import.startError) store.toast(t('内核已导入，但启动失败：{reason}', { reason: next.import.startError }), 'err')
          else store.toast(t('✅ 内核已手动导入到 {dir}', { dir: next.kernelDir }))
          onClose()
          void onChanged()
          return
        }
        await new Promise(resolve => setTimeout(resolve, 1000))
      }
      throw new Error(t('内核导入仍在后台执行，请刷新状态查看结果'))
    } catch (cause) {
      if (serverIdentity() === identity) setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      lock.current = false
      setBusy(false)
      if (serverIdentity() === identity) void onChanged()
    }
  }

  return <Sheet open onOpenChange={(open, details) => {
    if (open) return
    if (busy) { details.cancel(); return }
    onClose()
  }}>
    <SheetContent showCloseButton={!busy}>
      <SheetHeader><SheetTitle>{t('手动导入内核')}</SheetTitle></SheetHeader>
      <SheetBody className='flex flex-col gap-4'>
        <p className='text-xs leading-relaxed'>{t('请下载与网关平台一致的官方内核，不是浏览器所在设备的平台。')}</p>
        <p className='break-all font-mono text-xs'>{status.platform} / {status.arch} · {status.version}<br />{status.asset}</p>
        <Button variant='outline' disabled={!status.downloadUrl} onClick={() => openExternal(status.downloadUrl || '')}>{t('手动下载官方内核')}</Button>
        <p className='text-xs leading-relaxed text-subtle'>{t('支持官方压缩包和解压后的内核二进制；只接受当前版本，SHA-256 校验不通过时不会替换现有内核。')}</p>
        <Label>{t('导入来源')}</Label>
        <Select value={source} disabled={busy} onValueChange={value => { setSource(String(value)); setError(''); setConfirm(false) }}>
          <SelectTrigger><SelectValue>{options.find(option => option.value === source)?.label}</SelectValue></SelectTrigger>
          <SelectContent>{options.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
        </Select>
        {source === 'upload' ? <>
          <Input ref={inputRef} type='file' className='hidden' accept={kernelFileAccept(status)} disabled={busy} onChange={event => {
            const selected = event.currentTarget.files?.[0]
            if (!selected) return
            if (!selected.size || selected.size > limit) {
              setFile(null); setError(t('内核文件必须非空且不超过 {max} MiB', { max: Math.floor(limit / 1024 / 1024) })); return
            }
            setFile(selected); setError(''); setConfirm(false)
          }} />
          <Button variant='outline' disabled={busy} onClick={() => { if (inputRef.current) { inputRef.current.value = ''; inputRef.current.click() } }}>{t('选择内核文件…')}</Button>
          <p className='break-all text-xs'>{file ? `${file.name} · ${(file.size / 1024 / 1024).toFixed(1)} MiB` : t('未选择文件')}</p>
        </> : <>
          <Label htmlFor='mobile-kernel-path'>{t('文件或目录路径')}</Label>
          <Input id='mobile-kernel-path' disabled={busy} value={path} className='font-mono text-base' onChange={event => { setPath(event.currentTarget.value); setConfirm(false) }} />
          <p className='text-xs leading-relaxed text-subtle'>{t('先把下载的内核放到网关机器的目录，再填写文件或目录的绝对路径。目录内会查找官方包或 mihomo 二进制；远程面板不能读取浏览器电脑的路径。')}</p>
        </>}
        <Label htmlFor='mobile-kernel-dir'>{t('存放位置')}</Label>
        <Input id='mobile-kernel-dir' disabled={busy} value={dir} className='font-mono text-base' placeholder={status.kernelDirDefault || ''}
          onChange={event => { setDir(event.currentTarget.value); setConfirm(false) }} />
        <Button variant='ghost' disabled={busy} onClick={() => { setDir(status.kernelDirDefault || ''); setConfirm(false) }}>{t('恢复默认')}</Button>
        <p className='text-xs leading-relaxed text-subtle'>{t('这是网关机器上的安装目录，留空使用默认位置。导入会复制文件并建立版本目录，不删除原下载文件。')}</p>
        {confirm ? <Alert variant='warning'>{t('将替换现有代理内核，运行中的连接会短暂中断。订阅、账号引用与端口映射会保留。是否继续？')}</Alert> : null}
        {busy ? <><Progress value={null} /><p className='text-xs'>{phase}</p></> : null}
        {error ? <Alert variant='destructive'>{error}</Alert> : null}
      </SheetBody>
      <SheetFooter>
        <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={busy || current.installBusy || !status.asset} variant={confirm ? 'destructive' : 'default'}
          onClick={() => void install(confirm)}>{busy ? t('正在导入…') : confirm ? t('导入并替换') : t('导入内核')}</Button>
      </SheetFooter>
    </SheetContent>
  </Sheet>
}
