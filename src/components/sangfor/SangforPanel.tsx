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

// Retry budget for the window-waiting steps. Sangfor's client can be
// slow to cold-start (module loading, cert checks): budget ~60s at 500ms
// polls. When the client is already open, inject succeeds on attempt 1
// so the wall-clock cost is near zero.
const WINDOW_WAIT_ATTEMPTS = 120
const WINDOW_WAIT_DELAY_MS = 500

type BusyAction = 'launch' | 'inject' | 'click' | 'run-all' | null

export function SangforPanel() {
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
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
    const result = await commands.launchSangforClient()
    if (result.status === 'ok') {
      reportOk(result.data.message)
      logger.info('Sangfor launch result', result.data)
    } else {
      reportErr(result.error)
      logger.warn('Sangfor launch failed', { error: result.error })
    }
    setBusy(null)
  }

  const handleInject = async () => {
    const u = username.trim()
    if (!u || !password) {
      reportErr('请输入用户名和密码')
      return
    }
    setBusy('inject')
    const result = await commands.injectSangforCredentials(u, password)
    if (result.status === 'ok') {
      reportOk('账号已填入登录框')
    } else {
      reportErr(result.error)
      logger.warn('Sangfor inject failed', { error: result.error })
    }
    setBusy(null)
  }

  const handleClickLogin = async () => {
    setBusy('click')
    const result = await commands.clickSangforLogin()
    if (result.status === 'ok') {
      reportOk('登录按钮已点击')
    } else {
      reportErr(result.error)
      logger.warn('Sangfor login click failed', { error: result.error })
    }
    setBusy(null)
  }

  const handleRunAll = async () => {
    const u = username.trim()
    if (!u || !password) {
      reportErr('请输入用户名和密码')
      return
    }
    setBusy('run-all')

    // Step 1: launch (or focus if already running).
    reportOk('1/3 启动客户端…')
    const launchResult = await commands.launchSangforClient()
    if (launchResult.status !== 'ok') {
      reportErr(`启动失败: ${launchResult.error}`)
      logger.warn('run-all launch failed', { error: launchResult.error })
      setBusy(null)
      return
    }
    logger.info('run-all launch ok', launchResult.data)

    // Step 2: inject — retry until the login dialog is discoverable.
    // A cold-start needs a few seconds before the dialog exists; we poll
    // the inject command itself because it also owns the window lookup.
    const budgetS = Math.round(
      (WINDOW_WAIT_ATTEMPTS * WINDOW_WAIT_DELAY_MS) / 1000
    )
    reportOk(`2/3 等待登录窗口并填充账号… (最长 ${budgetS}s)`)
    const injectStartedAt = Date.now()
    const injectResult = await withRetry(
      () => commands.injectSangforCredentials(u, password),
      {
        maxAttempts: WINDOW_WAIT_ATTEMPTS,
        delayMs: WINDOW_WAIT_DELAY_MS,
        // Retry any "UI not yet ready" failure: window absent, Edit count
        // still growing, or target Edit not visible yet (tab control still
        // rendering). Non-transient errors (e.g. SendInput refused) bail
        // out immediately.
        shouldRetry: err =>
          err.includes('login window not found') ||
          err.includes('Expected at least') ||
          err.includes('Could not locate a visible'),
        onAttempt: attempt => {
          if (attempt > 1) {
            const waited = Math.round((Date.now() - injectStartedAt) / 1000)
            reportOk(`2/3 等待登录窗口… 已等待 ${waited}s / ${budgetS}s`)
          }
        },
      }
    )
    if (injectResult.status !== 'ok') {
      reportErr(`账号填充失败: ${injectResult.error}`)
      logger.warn('run-all inject failed', { error: injectResult.error })
      setBusy(null)
      return
    }

    // Step 3: click login. Small settle so the target app fully registers
    // the freshly-typed text before we fire BM_CLICK.
    await sleep(250)
    reportOk('3/3 提交登录…')
    const clickResult = await commands.clickSangforLogin()
    if (clickResult.status !== 'ok') {
      reportErr(`点击登录失败: ${clickResult.error}`)
      logger.warn('run-all click failed', { error: clickResult.error })
      setBusy(null)
      return
    }

    reportOk('一键登录完成')
    setBusy(null)
  }

  return (
    <Card className="w-full">
      <CardHeader>
        <CardTitle>Sangfor VDI 快速登录</CardTitle>
        <CardDescription>
          启动深信服桌面云客户端，自动填充账号并提交登录
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sangfor-username">用户名</Label>
            <Input
              id="sangfor-username"
              value={username}
              onChange={e => setUsername(e.target.value)}
              placeholder="请输入用户名"
              autoComplete="off"
              disabled={busy !== null}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="sangfor-password">密码</Label>
            <Input
              id="sangfor-password"
              type="password"
              value={password}
              onChange={e => setPassword(e.target.value)}
              autoComplete="off"
              disabled={busy !== null}
            />
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button onClick={handleRunAll} disabled={busy !== null}>
            {busy === 'run-all' ? '执行中…' : '一键登录'}
          </Button>
          <Button
            onClick={handleLaunch}
            disabled={busy !== null}
            variant="secondary"
          >
            {busy === 'launch' ? '启动中…' : '启动 / 激活客户端'}
          </Button>
          <Button
            onClick={handleInject}
            disabled={busy !== null}
            variant="secondary"
          >
            {busy === 'inject' ? '注入中…' : '自动填充账号'}
          </Button>
          <Button
            onClick={handleClickLogin}
            disabled={busy !== null}
            variant="secondary"
          >
            {busy === 'click' ? '点击中…' : '模拟登录'}
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
