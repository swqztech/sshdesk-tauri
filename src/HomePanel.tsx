import { useCallback, useEffect, useRef, useState } from 'react'
import type { HostPm2Snapshot, HostStats } from './types'
import { formatBytes, formatUptime, meterClass } from './hostStatsFormat'
import Pm2ProcessList from './Pm2ProcessList'

type Props = {
  connectionId: string
  hostTitle?: string
  active: boolean
  onOpenPm2Logs?: (processName: string) => void
}

export default function HomePanel({ connectionId, hostTitle, active, onOpenPm2Logs }: Props) {
  const [stats, setStats] = useState<HostStats | null>(null)
  const [pm2, setPm2] = useState<HostPm2Snapshot | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [lastRefresh, setLastRefresh] = useState(0)
  const refreshingRef = useRef(false)
  const lastPm2AtRef = useRef(0)

  const refresh = useCallback(async () => {
    if (!active || refreshingRef.current) return
    refreshingRef.current = true
    setBusy(true)
    setError('')
    const errors: string[] = []
    try {
      try {
        const data = await window.api.fetchHostStats(connectionId)
        setStats(data)
      } catch (err) {
        errors.push(err instanceof Error ? err.message : String(err))
      }
      const now = Date.now()
      if (now - lastPm2AtRef.current >= 10000) {
        try {
          const pm2Data = await window.api.fetchHostPm2(connectionId)
          setPm2(pm2Data)
          lastPm2AtRef.current = now
        } catch (err) {
          errors.push(`PM2: ${err instanceof Error ? err.message : String(err)}`)
        }
      }
      if (errors.length) setError(errors.join(' · '))
      else setError('')
      setLastRefresh(Date.now())
    } finally {
      setBusy(false)
      refreshingRef.current = false
    }
  }, [connectionId, active])

  useEffect(() => {
    if (!active) return
    void refresh()
    const timer = window.setInterval(() => void refresh(), 3000)
    return () => window.clearInterval(timer)
  }, [active, refresh])

  if (!active) return null

  return (
    <div className="mongo-panel home-panel">
      <header className="mongo-top">
        <div className="mongo-top-main">
          <h2>首页</h2>
          <p className="mongo-status">
            {hostTitle ? `${hostTitle} · ` : ''}
            {stats?.hostname || '—'}
            {stats ? ` · ${stats.os}` : ''}
          </p>
        </div>
        <div className="section-actions">
          <button type="button" className="ghost" disabled={busy} onClick={() => void refresh()}>
            {busy ? '刷新中…' : '立即刷新'}
          </button>
          {lastRefresh > 0 && (
            <span className="dim home-refresh-hint">
              状态每 3 秒 · PM2 每 10 秒 · {new Date(lastRefresh).toLocaleTimeString()}
            </span>
          )}
        </div>
      </header>

      {error && <div className="banner error file-banner">{error}</div>}

      {!stats && !error && <p className="empty">正在读取主机状态…</p>}

      {stats && (
        <div className="home-stack">
          <section className="home-metrics">
            <div className={`home-metric-card ${meterClass(stats.cpuPercent)}`}>
              <div className="home-metric-head">
                <span>CPU</span>
                <strong className={meterClass(stats.cpuPercent)}>
                  {stats.cpuPercent.toFixed(1)}%
                </strong>
              </div>
              <div className="home-meter">
                <div style={{ width: `${Math.min(100, stats.cpuPercent)}%` }} />
              </div>
              <p className="home-metric-sub">
                <span className="home-stat-num">{stats.cores}</span> 核 · 负载{' '}
                <span className="home-stat-num">{stats.load1.toFixed(2)}</span>
              </p>
            </div>

            <div className={`home-metric-card ${meterClass(stats.memPercent)}`}>
              <div className="home-metric-head">
                <span>内存</span>
                <strong className={meterClass(stats.memPercent)}>
                  {stats.memPercent.toFixed(1)}%
                </strong>
              </div>
              <div className="home-meter">
                <div style={{ width: `${Math.min(100, stats.memPercent)}%` }} />
              </div>
              <p className="home-metric-sub">
                <span className="home-stat-num">{formatBytes(stats.memUsedBytes)}</span>
                <span className="home-stat-sep"> / </span>
                <span className="home-stat-num">{formatBytes(stats.memTotalBytes)}</span>
              </p>
            </div>

            {stats.swapTotalBytes > 0 && (
              <div
                className={`home-metric-card ${meterClass(
                  (stats.swapUsedBytes / stats.swapTotalBytes) * 100
                )}`}
              >
                <div className="home-metric-head">
                  <span>交换分区</span>
                  <strong
                    className={meterClass(
                      (stats.swapUsedBytes / stats.swapTotalBytes) * 100
                    )}
                  >
                    {((stats.swapUsedBytes / stats.swapTotalBytes) * 100).toFixed(1)}%
                  </strong>
                </div>
                <div className="home-meter">
                  <div
                    style={{
                      width: `${Math.min(
                        100,
                        (stats.swapUsedBytes / stats.swapTotalBytes) * 100
                      )}%`
                    }}
                  />
                </div>
                <p className="home-metric-sub">
                  <span className="home-stat-num">{formatBytes(stats.swapUsedBytes)}</span>
                  <span className="home-stat-sep"> / </span>
                  <span className="home-stat-num">{formatBytes(stats.swapTotalBytes)}</span>
                </p>
              </div>
            )}
          </section>

          <section className="mongo-card">
            <div className="section-head">
              <h3>系统概览</h3>
            </div>
            <ul className="home-info-list">
              <li>
                <span>运行时间</span>
                <strong>{formatUptime(stats.uptimeSec)}</strong>
              </li>
              <li>
                <span>内核</span>
                <strong className="mono">{stats.kernel}</strong>
              </li>
              <li>
                <span>负载（1 / 5 / 15 分钟）</span>
                <strong className="mono">
                  {stats.load1.toFixed(2)} / {stats.load5.toFixed(2)} / {stats.load15.toFixed(2)}
                </strong>
              </li>
            </ul>
          </section>

          <section className="mongo-card">
            <div className="section-head">
              <h3>磁盘</h3>
              <p className="section-sub">挂载点占用（不含 tmpfs）</p>
            </div>
            {stats.disks.length === 0 ? (
              <p className="empty">未读取到磁盘信息</p>
            ) : (
              <ul className="mongo-list home-disk-list">
                {stats.disks.map((disk) => (
                  <li key={disk.mount}>
                    <div className="home-disk-main">
                      <strong className="mono home-disk-mount">{disk.mount}</strong>
                      <p className="home-disk-meta">
                        <span className="home-stat-num">{formatBytes(disk.usedBytes)}</span>
                        <span className="home-stat-sep"> / </span>
                        <span className="home-stat-num">{formatBytes(disk.sizeBytes)}</span>
                        <span className="home-stat-label"> · 可用 </span>
                        <span className="home-stat-num">{formatBytes(disk.availBytes)}</span>
                      </p>
                    </div>
                    <div className="home-disk-pct">
                      <div className={`home-meter compact ${meterClass(disk.usePercent)}`}>
                        <div style={{ width: `${Math.min(100, disk.usePercent)}%` }} />
                      </div>
                      <strong className={`home-disk-pct-num ${meterClass(disk.usePercent)}`}>
                        {disk.usePercent.toFixed(0)}%
                      </strong>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </section>

          <section className="mongo-card">
            <div className="section-head">
              <h3>PM2 进程</h3>
              <p className="section-sub">各进程 CPU / 内存占用（每 3 秒刷新）</p>
            </div>
            {pm2 ? (
              <Pm2ProcessList
                processes={pm2.processes}
                notice={pm2.notice}
                onOpenLogs={onOpenPm2Logs}
              />
            ) : (
              <p className="empty">正在读取 pm2…</p>
            )}
          </section>
        </div>
      )}
    </div>
  )
}
