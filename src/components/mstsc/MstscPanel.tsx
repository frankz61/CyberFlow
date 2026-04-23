import { useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { commands } from '@/lib/tauri-bindings'
import { logger } from '@/lib/logger'
import { sleep, withRetry } from '@/lib/retry'

// mstsc opens its dialog quickly (<1s typically). 15 attempts at 300ms
// gives us ~4.5s, which covers slow disks or UAC interruptions.
const WINDOW_WAIT_ATTEMPTS = 15
const WINDOW_WAIT_DELAY_MS = 300

type BusyAction = 'launch' | 'inject' | 'click' | 'run-all' | null

export function MstscPanel() {
  const [ip, setIp] = useState('')
  const [busy, setBusy] = useState<BusyAction>(null)
  const [status, setStatus] = useState<string | null>(null)
  const [statusKind, setStatusKind] = useState<'info' | 'error'>('info')

  const reportOk = (msg: string) => {
    setStatus(msg)
    setStatusKind('info')
  }
  const reportErr = (msg: string) => {
    setStatus(msg)
    setStatusKind('error')
  }

  const handleLaunch = async () => {
    setBusy('launch')
    const result = await commands.launchMstsc()
    if (result.status === 'ok') {
      reportOk(result.data.message)
      logger.info('mstsc launch result', result.data)
    } else {
      reportErr(result.error)
      logger.warn('mstsc launch failed', { error: result.error })
    }
    setBusy(null)
  }

  const handleInject = async () => {
    const trimmed = ip.trim()
    if (!trimmed) {
      reportErr('请输入 IP 或主机名')
      return
    }
    setBusy('inject')
    const result = await commands.injectMstscIp(trimmed)
    if (result.status === 'ok') {
      reportOk(`IP 已注入: ${trimmed}`)
    } else {
      reportErr(result.error)
      logger.warn('mstsc inject failed', { error: result.error })
    }
    setBusy(null)
  }

  const handleClickConnect = async () => {
    setBusy('click')
    const result = await commands.clickMstscConnect()
    if (result.status === 'ok') {
      reportOk('连接按钮已点击')
    } else {
      reportErr(result.error)
      logger.warn('mstsc connect click failed', { error: result.error })
    }
    setBusy(null)
  }

  const handleRunAll = async () => {
    const trimmed = ip.trim()
    if (!trimmed) {
      reportErr('请输入 IP 或主机名')
      return
    }
    setBusy('run-all')

    reportOk('1/3 启动 MSTSC…')
    const launchResult = await commands.launchMstsc()
    if (launchResult.status !== 'ok') {
      reportErr(`启动失败: ${launchResult.error}`)
      logger.warn('run-all launch failed', { error: launchResult.error })
      setBusy(null)
      return
    }
    logger.info('run-all launch ok', launchResult.data)

    reportOk('2/3 等待窗口并注入 IP…')
    const injectResult = await withRetry(
      () => commands.injectMstscIp(trimmed),
      {
        maxAttempts: WINDOW_WAIT_ATTEMPTS,
        delayMs: WINDOW_WAIT_DELAY_MS,
        shouldRetry: err =>
          err.includes('mstsc window not found') ||
          err.includes('No visible ComboBox') ||
          err.includes('No inner Edit'),
        onAttempt: attempt => {
          if (attempt > 1) {
            reportOk(`2/3 等待 MSTSC 窗口… (第 ${attempt} 次)`)
          }
        },
      }
    )
    if (injectResult.status !== 'ok') {
      reportErr(`IP 注入失败: ${injectResult.error}`)
      logger.warn('run-all inject failed', { error: injectResult.error })
      setBusy(null)
      return
    }

    await sleep(250)
    reportOk('3/3 提交连接…')
    const clickResult = await commands.clickMstscConnect()
    if (clickResult.status !== 'ok') {
      reportErr(`点击连接失败: ${clickResult.error}`)
      logger.warn('run-all click failed', { error: clickResult.error })
      setBusy(null)
      return
    }

    reportOk('一键连接完成')
    setBusy(null)
  }

  return (
    <Card className="w-full">
      <CardHeader>
        <CardTitle>Windows 远程桌面 (MSTSC)</CardTitle>
        <CardDescription>
          启动 mstsc，将 IP / 主机名注入"计算机"字段，然后点击连接
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <div className="flex items-end gap-2">
          <div className="flex flex-col gap-1.5 flex-1 min-w-0">
            <Label htmlFor="mstsc-ip">IP / 主机名</Label>
            <Input
              id="mstsc-ip"
              value={ip}
              onChange={e => setIp(e.target.value)}
              placeholder="例如 192.168.1.100"
              disabled={busy !== null}
            />
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button onClick={handleRunAll} disabled={busy !== null}>
            {busy === 'run-all' ? '执行中…' : '一键连接'}
          </Button>
          <Button
            onClick={handleLaunch}
            disabled={busy !== null}
            variant="secondary"
          >
            {busy === 'launch' ? '启动中…' : '启动 MSTSC'}
          </Button>
          <Button
            onClick={handleInject}
            disabled={busy !== null}
            variant="secondary"
          >
            {busy === 'inject' ? '注入中…' : '注入 IP'}
          </Button>
          <Button
            onClick={handleClickConnect}
            disabled={busy !== null}
            variant="secondary"
          >
            {busy === 'click' ? '点击中…' : '模拟连接'}
          </Button>
        </div>
        {status && (
          <p
            className={
              statusKind === 'error'
                ? 'text-sm text-destructive'
                : 'text-sm text-muted-foreground'
            }
          >
            {status}
          </p>
        )}
      </CardContent>
    </Card>
  )
}
