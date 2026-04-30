import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { commands } from '@/lib/tauri-bindings'
import type { ServerInfo } from '@/lib/bindings'
import { logger } from '@/lib/logger'

type BusyAction = 'start' | 'stop' | 'regenerate' | null

function buildMcpConfig(info: ServerInfo): string {
  // Shape commonly accepted by MCP clients (Claude Desktop config.json etc.)
  return JSON.stringify(
    {
      mcpServers: {
        cyberflow: {
          url: info.url,
          headers: { Authorization: `Bearer ${info.token}` },
        },
      },
    },
    null,
    2
  )
}

async function copyToClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch (err) {
    logger.warn('clipboard write failed', { err: String(err) })
    return false
  }
}

export function McpServerPanel() {
  const [info, setInfo] = useState<ServerInfo | null>(null)
  const [busy, setBusy] = useState<BusyAction>(null)
  const [status, setStatus] = useState<string | null>(null)
  const [statusKind, setStatusKind] = useState<'info' | 'error'>('info')
  const [tokenVisible, setTokenVisible] = useState(false)

  useEffect(() => {
    commands.getMcpServerStatus().then(result => {
      if (result.status === 'ok') setInfo(result.data)
    })
  }, [])

  const reportOk = (msg: string) => {
    setStatus(msg)
    setStatusKind('info')
  }
  const reportErr = (msg: string) => {
    setStatus(msg)
    setStatusKind('error')
  }

  const handleStart = async () => {
    setBusy('start')
    const result = await commands.startMcpServer()
    if (result.status === 'ok') {
      setInfo(result.data)
      reportOk(
        result.data.running
          ? `MCP 服务已启动 (${result.data.url})`
          : 'MCP 服务启动响应异常'
      )
      logger.info('mcp server started', result.data)
    } else {
      reportErr(result.error)
      logger.warn('mcp start failed', { error: result.error })
    }
    setBusy(null)
  }

  const handleStop = async () => {
    setBusy('stop')
    const result = await commands.stopMcpServer()
    if (result.status === 'ok') {
      setInfo(result.data)
      reportOk('MCP 服务已停止')
    } else {
      reportErr(result.error)
      logger.warn('mcp stop failed', { error: result.error })
    }
    setBusy(null)
  }

  const handleRegenerate = async () => {
    setBusy('regenerate')
    const result = await commands.regenerateMcpToken()
    if (result.status === 'ok') {
      setInfo(result.data)
      reportOk('Token 已重置，请更新客户端配置')
    } else {
      reportErr(result.error)
      logger.warn('mcp regenerate failed', { error: result.error })
    }
    setBusy(null)
  }

  const handleCopyConfig = async () => {
    if (!info) return
    const ok = await copyToClipboard(buildMcpConfig(info))
    if (ok) reportOk('MCP 客户端配置已复制到剪贴板')
    else reportErr('复制失败，请手动选中文本')
  }

  const handleCopyToken = async () => {
    if (!info) return
    const ok = await copyToClipboard(info.token)
    if (ok) reportOk('Token 已复制到剪贴板')
    else reportErr('复制失败')
  }

  return (
    <Card className="w-full">
      <CardHeader>
        <CardTitle>MCP 服务（本地）</CardTitle>
        <CardDescription>
          将一键流程作为 MCP 工具暴露到本地 HTTP 接口，供 Claude Desktop /
          Cursor 等客户端调用
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 text-sm">
          <span className="text-muted-foreground">状态</span>
          <span
            className={
              info?.running
                ? 'text-green-600 dark:text-green-400 font-medium'
                : 'text-muted-foreground'
            }
          >
            {info?.running ? '运行中' : '已停止'}
          </span>

          <span className="text-muted-foreground">URL</span>
          <span className="font-mono text-xs break-all">
            {info?.url ?? '—'}
          </span>

          <span className="text-muted-foreground">Token</span>
          <span className="flex items-center gap-2 min-w-0">
            <code className="font-mono text-xs break-all flex-1">
              {tokenVisible
                ? (info?.token ?? '—')
                : info?.token
                  ? '•'.repeat(Math.min(32, info.token.length))
                  : '—'}
            </code>
            <button
              type="button"
              onClick={() => setTokenVisible(v => !v)}
              className="text-xs text-muted-foreground hover:text-foreground shrink-0"
            >
              {tokenVisible ? '隐藏' : '显示'}
            </button>
          </span>

          <span className="text-muted-foreground">暴露的工具</span>
          <span className="font-mono text-xs">
            sangfor_login, mstsc_connect
          </span>
        </div>

        <div className="flex flex-wrap gap-2">
          {info?.running ? (
            <Button onClick={handleStop} disabled={busy !== null}>
              {busy === 'stop' ? '停止中…' : '停止服务'}
            </Button>
          ) : (
            <Button onClick={handleStart} disabled={busy !== null}>
              {busy === 'start' ? '启动中…' : '启动服务'}
            </Button>
          )}
          <Button
            onClick={handleCopyConfig}
            disabled={busy !== null || !info}
            variant="secondary"
          >
            复制 MCP 配置
          </Button>
          <Button
            onClick={handleCopyToken}
            disabled={busy !== null || !info}
            variant="secondary"
          >
            复制 Token
          </Button>
          <Button
            onClick={handleRegenerate}
            disabled={busy !== null}
            variant="outline"
          >
            {busy === 'regenerate' ? '重置中…' : '重置 Token'}
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
