import type { Pm2Process } from './types'
import { formatBytes, formatUptime, meterClass } from './hostStatsFormat'

type Props = {
  processes: Pm2Process[]
  notice?: string
  compact?: boolean
  onOpenLogs?: (processName: string) => void
}

function statusClass(status: string) {
  const s = status.toLowerCase()
  if (s === 'online' || s === 'launching') return 'ok'
  if (s === 'stopped' || s === 'stopping') return 'muted'
  return 'danger'
}

function statusLabel(status: string) {
  const map: Record<string, string> = {
    online: '运行中',
    stopped: '已停止',
    stopping: '停止中',
    launching: '启动中',
    errored: '异常',
    waiting_restart: '等待重启'
  }
  return map[status.toLowerCase()] || status
}

export default function Pm2ProcessList({ processes, notice, compact, onOpenLogs }: Props) {
  if (!processes.length) {
    return <p className="empty pm2-empty">{notice || '暂无 pm2 进程'}</p>
  }

  return (
    <div className={`pm2-block ${compact ? 'compact' : ''}`}>
      {notice && <p className="dim pm2-notice">{notice}</p>}
      <table className="pm2-table">
        <thead>
          <tr>
            <th>进程</th>
            <th>状态</th>
            <th>运行时长</th>
            <th>CPU</th>
            <th>内存</th>
            {!compact && <th>PID</th>}
            {!compact && <th>重启</th>}
          </tr>
        </thead>
        <tbody>
          {processes.map((proc) => (
            <tr key={`${proc.pmId}-${proc.name}`}>
              <td className="mono pm2-name" title={proc.name}>
                {onOpenLogs ? (
                  <button
                    type="button"
                    className="pm2-name-btn"
                    onClick={() => onOpenLogs(proc.name)}
                    title={`pm2 logs ${proc.name}`}
                  >
                    {proc.name}
                  </button>
                ) : (
                  proc.name
                )}
              </td>
              <td>
                <span className={`pm2-status ${statusClass(proc.status)}`}>
                  {statusLabel(proc.status)}
                </span>
              </td>
              <td className="mono pm2-num pm2-uptime">
                {proc.uptimeSec > 0 ? formatUptime(proc.uptimeSec) : '—'}
              </td>
              <td className={`mono pm2-num ${meterClass(proc.cpuPercent)}`}>
                {proc.cpuPercent.toFixed(1)}%
              </td>
              <td className="mono pm2-num">{formatBytes(proc.memoryBytes)}</td>
              {!compact && <td className="mono pm2-num">{proc.pid || '—'}</td>}
              {!compact && <td className="mono pm2-num">{proc.restarts}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
