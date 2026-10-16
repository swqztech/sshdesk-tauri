import { useCallback, useEffect, useRef, useState } from 'react'
import type { HostPm2Snapshot, HostStats, HostView } from './types'
import { formatBytes, formatUptime, meterClass } from './hostStatsFormat'
import Pm2ProcessList from './Pm2ProcessList'

type HostTabRef = {
  connectionId: string
  hostId: string
  title: string
}

type HostRow = {
  connectionId: string
  label: string
  tip: string
  stats: HostStats | null
  pm2: HostPm2Snapshot | null
  error: string
}

type Props = {
  connections: HostTabRef[]
  hosts: HostView[]
  activeConnectionId: string | null
  active: boolean
  onSelect: (connectionId: string) => void
  onOpenPm2Logs?: (connectionId: string, processName: string) => void
}

function rootDisk(stats: HostStats | null) {
  if (!stats?.disks?.length) return null
  return stats.disks.find((d) => d.mount === '/') ?? stats.disks[0]
}

export default function OverviewPanel({
  connections,
  hosts,
  activeConnectionId,
  active,
  onSelect,
  onOpenPm2Logs
}: Props) {
  const [rows, setRows] = useState<HostRow[]>([])
  const [busy, setBusy] = useState(false)
  const [lastRefresh, setLastRefresh] = useState(0)
  const refreshingRef = useRef(false)
  const lastPm2AtRef = useRef(0)

  const refresh = useCallback(async () => {
    if (!active || connections.length === 0 || refreshingRef.current) return
    refreshingRef.current = true
    setBusy(true)
    try {
    const now = Date.now()
    const fetchPm2 = now - lastPm2AtRef.current >= 10000
    const results = await Promise.all(
      connections.map(async (tab) => {
        const host = hosts.find((h) => h.id === tab.hostId)
        const label = host?.name || tab.title
        const tip = host ? `${host.username}@${host.host}:${host.port}` : tab.title
        let stats: HostStats | null = null
        let pm2: HostPm2Snapshot | null = null
        const errors: string[] = []

        try {
          stats = await window.api.fetchHostStats(tab.connectionId)
        } catch (err) {
          errors.push(err instanceof Error ? err.message : String(err))
        }

        if (fetchPm2) {
          try {
            pm2 = await window.api.fetchHostPm2(tab.connectionId)
          } catch (err) {
            errors.push(`PM2: ${err instanceof Error ? err.message : String(err)}`)
          }
        }

        return {
          connectionId: tab.connectionId,
          label,
          tip,
          stats,
          pm2,
          error: errors.join(' · ')
        }
      })
    )
    if (fetchPm2) lastPm2AtRef.current = now
    setRows((prev) =>
      results.map((row) => {
        if (row.pm2) return row
        const old = prev.find((p) => p.connectionId === row.connectionId)
        return old?.pm2 ? { ...row, pm2: old.pm2 } : row
      })
    )
    setLastRefresh(Date.now())
    } finally {
      setBusy(false)
      refreshingRef.current = false
    }
  }, [active, connections, hosts])

  useEffect(() => {
    if (!active) return
    void refresh()
    const timer = window.setInterval(() => void refresh(), 3000)
    return () => window.clearInterval(timer)
  }, [active, refresh])

  if (!active) return null

  return (
    <div className="mongo-panel home-panel overview-panel">
      <header className="mongo-top">
        <div className="mongo-top-main">
          <h2>总览</h2>
          <p className="mongo-status">
            同时监控 {connections.length} 台已连接主机 · 状态每 3 秒 · PM2 每 10 秒
          </p>
        </div>
        <div className="section-actions">
          <button type="button" className="ghost" disabled={busy} onClick={() => void refresh()}>
            {busy ? '刷新中…' : '立即刷新'}
          </button>
          {lastRefresh > 0 && (
            <span className="dim home-refresh-hint">{new Date(lastRefresh).toLocaleTimeString()}</span>
          )}
        </div>
      </header>

      {connections.length === 0 && (
        <p className="empty">请先连接至少一台主机，即可在此同屏查看 CPU / 内存。</p>
      )}

      <div className="overview-grid">
        {rows.map((row) => {
          const disk = rootDisk(row.stats)
          const isActive = row.connectionId === activeConnectionId
          return (
            <section
              key={row.connectionId}
              className={`overview-host-card ${isActive ? 'active' : ''}`}
            >
              <div className="overview-host-head">
                <div>
                  <h3>{row.label}</h3>
                  <p className="mono dim">{row.tip}</p>
                  {row.stats && (
                    <p className="dim overview-host-sub">
                      {row.stats.hostname} · {row.stats.os}
                    </p>
                  )}
                </div>
                <button
                  type="button"
                  className="ghost"
                  onClick={() => onSelect(row.connectionId)}
                >
                  查看详情
                </button>
              </div>

              {row.error && <div className="banner error file-banner">{row.error}</div>}

              {!row.stats && !row.error && <p className="empty">读取中…</p>}

              {row.stats && (
                <div className="overview-metrics">
                  <div className={`overview-metric ${meterClass(row.stats.cpuPercent)}`}>
                    <div className="overview-metric-top">
                      <span>CPU</span>
                      <strong className={meterClass(row.stats.cpuPercent)}>
                        {row.stats.cpuPercent.toFixed(1)}%
                      </strong>
                    </div>
                    <div className="home-meter">
                      <div style={{ width: `${Math.min(100, row.stats.cpuPercent)}%` }} />
                    </div>
                    <p className="overview-metric-sub">
                      {row.stats.cores} 核 · 负载 {row.stats.load1.toFixed(2)}
                    </p>
                  </div>

                  <div className={`overview-metric ${meterClass(row.stats.memPercent)}`}>
                    <div className="overview-metric-top">
                      <span>内存</span>
                      <strong className={meterClass(row.stats.memPercent)}>
                        {row.stats.memPercent.toFixed(1)}%
                      </strong>
                    </div>
                    <div className="home-meter">
                      <div style={{ width: `${Math.min(100, row.stats.memPercent)}%` }} />
                    </div>
                    <p className="overview-metric-sub">
                      {formatBytes(row.stats.memUsedBytes)} / {formatBytes(row.stats.memTotalBytes)}
                    </p>
                  </div>
                </div>
              )}

              {row.stats && (
                <ul className="overview-foot">
                  <li>
                    <span>运行</span>
                    <strong>{formatUptime(row.stats.uptimeSec)}</strong>
                  </li>
                  <li>
                    <span>负载 1/5/15</span>
                    <strong className="mono">
                      {row.stats.load1.toFixed(2)} / {row.stats.load5.toFixed(2)} /{' '}
                      {row.stats.load15.toFixed(2)}
                    </strong>
                  </li>
                  {disk && (
                    <li>
                      <span>磁盘 {disk.mount}</span>
                      <strong className={meterClass(disk.usePercent)}>
                        {disk.usePercent.toFixed(0)}%
                      </strong>
                    </li>
                  )}
                </ul>
              )}

              <div className="overview-pm2">
                <div className="overview-pm2-head">
                  <strong>PM2 进程</strong>
                  {row.pm2 && row.pm2.processes.length > 0 && (
                    <span className="dim">{row.pm2.processes.length} 个</span>
                  )}
                </div>
                {row.pm2 ? (
                  <Pm2ProcessList
                    processes={row.pm2.processes}
                    notice={row.pm2.notice}
                    compact
                    onOpenLogs={
                      onOpenPm2Logs
                        ? (name) => onOpenPm2Logs(row.connectionId, name)
                        : undefined
                    }
                  />
                ) : (
                  !row.error && <p className="empty pm2-empty">读取 pm2…</p>
                )}
              </div>
            </section>
          )
        })}
      </div>
    </div>
  )
}
