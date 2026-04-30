import { cn } from '@/lib/utils'
import { SangforPanel } from '@/components/sangfor/SangforPanel'
import { MstscPanel } from '@/components/mstsc/MstscPanel'
import { McpServerPanel } from '@/components/mcp/McpServerPanel'

interface MainWindowContentProps {
  children?: React.ReactNode
  className?: string
}

export function MainWindowContent({
  children,
  className,
}: MainWindowContentProps) {
  return (
    <div className={cn('flex h-full flex-col bg-background', className)}>
      {children || (
        <div className="flex-1 overflow-y-auto">
          <div className="mx-auto flex w-full max-w-3xl flex-col gap-6 p-6">
            <header>
              <h1 className="text-2xl font-bold text-foreground">
                自动化工作台
              </h1>
              <p className="text-sm text-muted-foreground">
                按栏目分组的桌面自动化快捷入口
              </p>
            </header>
            <McpServerPanel />
            <SangforPanel />
            <MstscPanel />
          </div>
        </div>
      )}
    </div>
  )
}
