import { useEffect, useMemo, useState } from 'react'
import type {
  BackupConfig,
  LocalBackupItem,
  MongoConn,
  MongoDbInfo,
  MongoLocalConn,
  OssBackupItem
} from './types'

type Props = {
  sessionId: string
  active: boolean
  hostTitle?: string
}

type Tab = 'files' | 'backup' | 'settings'

function formatBytes(n: unknown) {
  const bytes = typeof n === 'number' ? n : Number(n)
  if (!Number.isFinite(bytes) || bytes < 0) return '—'
  if (bytes < 1024) return `${Math.round(bytes)} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(2)} MB`
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`
}

function formatTime(ms: number | string) {
  const d = typeof ms === 'number' ? new Date(ms) : new Date(ms)
  if (Number.isNaN(d.getTime())) return '—'
  return d.toLocaleString()
}

function parseSourceDbFromFileName(fileName: string): string | null {
  const base = fileName.replace(/^.*[/\\]/, '')
  const matched = base.match(/^(.+)_bson_(?:incr_)?\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}/)
  return matched?.[1] ?? null
}

function isIncrementalBackupName(name: string) {
  return /_bson_incr_\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}_mc\.zip$/i.test(name)
}

function backupKindLabel(name: string) {
  return isIncrementalBackupName(name) ? '增量' : '全量'
}

const emptyCfg = (): BackupConfig => ({
  tukuBackendRoot: '',
  mongodbUrl: '',
  mongoBackupDb: 'tuku',
  mongoBackupDir: '/var/backups/mongodb',
  mongoBackupRetainDays: 7,
  ossAccessKeyId: '',
  ossAccessKeySecret: '',
  ossBucket: '',
  ossRegion: 'oss-cn-shenzhen',
  ossEndpoint: 'oss-cn-shenzhen.aliyuncs.com',
  ossInternal: true,
  ossBackupRoot: 'backups',
  ossMongodbPrefix: 'backups/mongodb',
  ossMongodbRetainDays: 14,
  configPath: '/etc/default/tuku-backup'
})

export default function MongoPanel({ sessionId, active, hostTitle }: Props) {
  const [tab, setTab] = useState<Tab>('files')
  const [conn, setConn] = useState<MongoConn>({
    host: '127.0.0.1',
    port: 27017,
    username: '',
    password: '',
    authDb: 'admin'
  })
  const [cfg, setCfg] = useState<BackupConfig>(emptyCfg)
  const [dbs, setDbs] = useState<MongoDbInfo[]>([])
  const [localBackups, setLocalBackups] = useState<LocalBackupItem[]>([])
  const [pcBackups, setPcBackups] = useState<LocalBackupItem[]>([])
  const [ossBackups, setOssBackups] = useState<OssBackupItem[]>([])
  const [localDir, setLocalDir] = useState('')
  const [busy, setBusy] = useState('')
  const [toast, setToast] = useState<{ kind: 'ok' | 'err'; text: string; path?: string } | null>(
    null
  )
  const [progress, setProgress] = useState<{
    message: string
    percent: number
    label: string
  } | null>(null)
  const [connOpen, setConnOpen] = useState(false)
  const [restoreDlg, setRestoreDlg] = useState<{
    item: LocalBackupItem
    targetDb: string
    mode: 'merge' | 'replace'
    sourceHint: string | null
    localConn: MongoLocalConn
    backupKind: 'full' | 'incremental' | 'unknown'
    chainHint?: string
  } | null>(null)
  const [incrReady, setIncrReady] = useState<Record<string, boolean>>({})

  const ossReady = Boolean(cfg.ossAccessKeyId && cfg.ossAccessKeySecret && cfg.ossBucket)

  const ossNameSet = useMemo(() => new Set(ossBackups.map((o) => o.name)), [ossBackups])

  const mongoTargetHint = useMemo(() => {
    const uri = cfg.mongodbUrl.trim()
    if (uri) {
      return uri.replace(/\/\/([^:@/]+):([^@/]+)@/, '//$1:***@')
    }
    return `服务器本机 ${conn.host || '127.0.0.1'}:${conn.port ?? 27017}`
  }, [cfg.mongodbUrl, conn.host, conn.port])

  const statusLine = useMemo(() => {
    const parts = [
      hostTitle ? `远程 ${hostTitle}` : '远程 Mongo',
      cfg.mongodbUrl.trim() ? 'MONGODB_URL' : mongoTargetHint,
      ossReady ? `OSS · ${cfg.ossBucket}` : 'OSS 未配置',
      localDir ? '已选下载目录' : '下载时再选目录'
    ]
    return parts.join('  ·  ')
  }, [hostTitle, mongoTargetHint, cfg.mongodbUrl, ossReady, cfg.ossBucket, localDir])

  useEffect(() => {
    return window.api.onBackupProgress((p) => {
      if (p.sessionId !== sessionId) return
      setProgress({ message: p.message, percent: p.percent, label: p.dbName })
    })
  }, [sessionId])

  useEffect(() => {
    if (!active) return
    void (async () => {
      setBusy('boot')
      setToast(null)
      try {
        const loaded = await window.api.loadBackupConfig(sessionId)
        setCfg(loaded)
        const [local, oss] = await Promise.all([
          window.api.listLocalBackups(sessionId, loaded),
          loaded.ossAccessKeyId && loaded.ossAccessKeySecret && loaded.ossBucket
            ? window.api.listOssBackups(loaded)
            : Promise.resolve([] as OssBackupItem[])
        ])
        setLocalBackups(local)
        setOssBackups(oss)
        const dl = await window.api.defaultDownloadDir()
        setLocalDir((prev) => prev || dl)
        setPcBackups(await window.api.listPcMongoBackups(dl))
      } catch (err) {
        const fallback = await window.api.defaultBackupConfig()
        setCfg(fallback)
        setToast({ kind: 'err', text: err instanceof Error ? err.message : String(err) })
      } finally {
        setBusy('')
      }
    })()
  }, [sessionId, active])

  if (!active) return null

  function showOk(text: string, path?: string) {
    setToast({ kind: 'ok', text, path })
  }

  function showErr(err: unknown) {
    setToast({ kind: 'err', text: err instanceof Error ? err.message : String(err) })
  }

  async function refreshDbs() {
    setBusy('list')
    setToast(null)
    try {
      const list = await window.api.listMongoDatabases(sessionId, conn, cfg)
      setDbs(list)
      const ready: Record<string, boolean> = {}
      await Promise.all(
        list.map(async (db) => {
          try {
            const chain = await window.api.loadMongoBackupChain(sessionId, db.name, cfg)
            ready[db.name] = Boolean(chain?.entries?.length)
          } catch {
            ready[db.name] = false
          }
        })
      )
      setIncrReady(ready)
    } catch (err) {
      showErr(err)
      setDbs([])
      setIncrReady({})
    } finally {
      setBusy('')
    }
  }

  async function refreshOssBackups(nextCfg = cfg) {
    setBusy('ossList')
    setToast(null)
    try {
      if (!nextCfg.ossAccessKeyId || !nextCfg.ossAccessKeySecret || !nextCfg.ossBucket) {
        setOssBackups([])
        setToast({ kind: 'err', text: '请先在「设置」里填写 OSS' })
        return
      }
      setOssBackups(await window.api.listOssBackups(nextCfg))
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function refreshPcBackups(dir = localDir) {
    setBusy('pcList')
    setToast(null)
    try {
      const root = dir || (await window.api.defaultDownloadDir())
      if (!dir) setLocalDir(root)
      setPcBackups(await window.api.listPcMongoBackups(root))
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function refreshLocalBackups(nextCfg = cfg) {
    setBusy('localList')
    setToast(null)
    try {
      setLocalBackups(await window.api.listLocalBackups(sessionId, nextCfg))
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function refreshAllBackups(nextCfg = cfg) {
    setBusy('backups')
    setToast(null)
    try {
      const [local, oss] = await Promise.all([
        window.api.listLocalBackups(sessionId, nextCfg),
        nextCfg.ossAccessKeyId && nextCfg.ossAccessKeySecret && nextCfg.ossBucket
          ? window.api.listOssBackups(nextCfg)
          : Promise.resolve([] as OssBackupItem[])
      ])
      setLocalBackups(local)
      setOssBackups(oss)
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function saveCfg() {
    setBusy('saveCfg')
    setToast(null)
    try {
      const saved = await window.api.saveBackupConfig(sessionId, cfg)
      setCfg(saved)
      showOk(`配置已保存到 ${saved.configPath}`)
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function backup(dbName: string) {
    setBusy(`backup:${dbName}`)
    setToast(null)
    setProgress({ message: '准备全量备份…', percent: 2, label: dbName })
    try {
      const res = await window.api.backupMongo(sessionId, dbName, conn, cfg)
      setProgress({
        message: res.uploaded ? '已上传 OSS' : '仅保留在服务器',
        percent: 100,
        label: dbName
      })
      showOk(
        res.uploaded
          ? `全量 ${res.fileName} 已上传 OSS`
          : `全量 ${res.fileName} 已写到服务器（未上传 OSS）`
      )
      setIncrReady((m) => ({ ...m, [dbName]: true }))
      await refreshAllBackups()
      setTab('files')
    } catch (err) {
      showErr(err)
      setProgress(null)
    } finally {
      setBusy('')
    }
  }

  async function backupIncremental(dbName: string) {
    setBusy(`incr:${dbName}`)
    setToast(null)
    setProgress({ message: '准备增量备份…', percent: 2, label: dbName })
    try {
      const res = await window.api.backupMongoIncremental(sessionId, dbName, conn, cfg)
      setProgress({
        message: res.uploaded ? '已上传 OSS' : '仅保留在服务器',
        percent: 100,
        label: dbName
      })
      showOk(
        res.uploaded
          ? `增量 ${res.fileName}（${res.changedCollections.length} 个集合）已上传 OSS`
          : `增量 ${res.fileName}（${res.changedCollections.length} 个集合）已写到服务器`
      )
      await refreshAllBackups()
      setTab('files')
    } catch (err) {
      showErr(err)
      setProgress(null)
    } finally {
      setBusy('')
    }
  }

  async function downloadOss(item: OssBackupItem) {
    setBusy(`dl:${item.key}`)
    setToast(null)
    setProgress({ message: '准备下载…', percent: 2, label: item.name })
    try {
      const dir = localDir || (await window.api.pickDownloadDir()) || ''
      if (!dir) {
        setToast({ kind: 'err', text: '请先选择本机下载目录' })
        setProgress(null)
        return
      }
      if (!localDir) setLocalDir(dir)
      const res = await window.api.downloadOssBackup(sessionId, cfg, item.key, dir)
      setProgress({ message: '下载完成', percent: 100, label: item.name })
      showOk(`${res.fileName}（${formatBytes(res.bytes)}）`, res.localPath)
      await refreshPcBackups(dir)
    } catch (err) {
      showErr(err)
      setProgress(null)
    } finally {
      setBusy('')
    }
  }

  async function removeLocal(item: LocalBackupItem) {
    if (!confirm(`删除服务器文件？\n${item.name}`)) return
    setBusy(`rmL:${item.path}`)
    setToast(null)
    try {
      await window.api.deleteLocalBackup(sessionId, item.path)
      await refreshLocalBackups()
      showOk(`已删除 ${item.name}`)
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function removeOss(item: OssBackupItem) {
    if (!confirm(`删除 OSS 对象？\n${item.name}`)) return
    setBusy(`rmO:${item.key}`)
    setToast(null)
    try {
      await window.api.deleteOssBackup(cfg, item.key)
      await refreshOssBackups()
      showOk(`已删除 ${item.name}`)
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function uploadServerToOss(item: LocalBackupItem) {
    if (!ossReady) {
      setToast({ kind: 'err', text: '请先在「设置」里填写 OSS' })
      return
    }
    setBusy(`upOss:${item.path}`)
    setToast(null)
    setProgress({ message: '准备上传…', percent: 2, label: item.name })
    try {
      const res = await window.api.uploadServerBackupToOss(sessionId, item.path, cfg)
      const viaLabel =
        res.via === 'server-internal'
          ? '服务器内网'
          : res.via === 'server-public'
            ? '服务器公网'
            : '本机中转'
      setProgress({ message: '上传完成', percent: 100, label: item.name })
      showOk(`${item.name} 已上传 OSS（${viaLabel}）`)
      await refreshOssBackups()
    } catch (err) {
      showErr(err)
      setProgress(null)
    } finally {
      setBusy('')
    }
  }

  function openRestore(item: LocalBackupItem) {
    const hint = parseSourceDbFromFileName(item.name)
    setToast(null)
    void (async () => {
      const localConn = await window.api.defaultLocalMongoConn()
      const meta = await window.api.readMongoBackupMeta(item.path)
      const backupKind =
        meta?.type ?? (isIncrementalBackupName(item.name) ? 'incremental' : 'full')
      const chainHint =
        backupKind === 'incremental'
          ? `增量包：将自动合并全量基线及中间增量（需同目录下有基线 ${meta?.baseBackupId || '…'}）`
          : undefined
      setRestoreDlg({
        item,
        targetDb: hint || cfg.mongoBackupDb || 'tuku',
        mode: 'merge',
        sourceHint: hint,
        localConn,
        backupKind,
        chainHint
      })
    })()
  }

  async function confirmRestore() {
    if (!restoreDlg) return
    const { item, targetDb, mode, sourceHint, localConn } = restoreDlg
    const db = targetDb.trim()
    if (!db) {
      setToast({ kind: 'err', text: '请填写目标数据库名' })
      return
    }
    setRestoreDlg(null)
    setBusy(`restore:${item.path}`)
    setToast(null)
    setProgress({ message: '准备恢复到本机…', percent: 2, label: db })
    try {
      const res = await window.api.restoreMongoFromLocal(
        sessionId,
        item.path,
        db,
        mode,
        localConn,
        sourceHint,
        pcBackups
      )
      setProgress({ message: '恢复完成', percent: 100, label: db })
      const chainNote = res.steps && res.steps > 1 ? `（链式 ${res.steps} 步）` : ''
      showOk(
        `已恢复到本机库 ${res.db}${chainNote}（${res.connectionLabel} · ${mode === 'replace' ? '覆盖' : '合并'}）`
      )
    } catch (err) {
      showErr(err)
      setProgress(null)
    } finally {
      setBusy('')
    }
  }

  async function pickDir() {
    const d = await window.api.pickDownloadDir()
    if (d) {
      setLocalDir(d)
      showOk('已设置本机下载目录', d)
      await refreshPcBackups(d)
    }
  }

  return (
    <div className="mongo-panel">
      <header className="mongo-top">
        <div className="mongo-top-main">
          <h2>MongoDB</h2>
          <p className="mongo-status">{busy === 'boot' ? '加载配置中…' : statusLine}</p>
        </div>
        <nav className="mongo-tabs" aria-label="Mongo 分区">
          {(
            [
              ['files', '备份'],
              ['backup', '新建'],
              ['settings', '设置']
            ] as const
          ).map(([id, label]) => (
            <button
              key={id}
              type="button"
              className={tab === id ? 'on' : ''}
              onClick={() => setTab(id)}
            >
              {label}
            </button>
          ))}
        </nav>
      </header>

      {(progress || toast) && (
        <div className="mongo-flash">
          {progress && (
            <div className="backup-progress">
              <div className="backup-meta">
                <strong>{progress.label}</strong>
                <span>{progress.percent}%</span>
              </div>
              <div className="progress-track">
                <div style={{ width: `${progress.percent}%` }} />
              </div>
              <p className="dim">{progress.message}</p>
            </div>
          )}
          {toast && (
            <div className={`mongo-toast ${toast.kind === 'ok' ? 'ok' : 'err'}`}>
              <span>{toast.text}</span>
              {toast.path && (
                <button
                  type="button"
                  className="ghost"
                  onClick={() => void window.api.showItem(toast.path!)}
                >
                  打开位置
                </button>
              )}
              <button type="button" className="ghost" onClick={() => setToast(null)}>
                关闭
              </button>
            </div>
          )}
        </div>
      )}

      {tab === 'files' && (
        <div className="mongo-stack">
          <section className="mongo-card">
            <div className="section-head">
              <div>
                <h3>OSS</h3>
                <p className="section-sub">从此下载更快</p>
              </div>
              <div className="section-actions">
                <button type="button" className="ghost" disabled={Boolean(busy)} onClick={() => void pickDir()}>
                  {localDir ? '下载目录' : '选下载目录'}
                </button>
                <button
                  type="button"
                  className="ghost"
                  disabled={Boolean(busy)}
                  onClick={() => void refreshOssBackups()}
                >
                  {busy === 'ossList' ? '…' : '刷新'}
                </button>
              </div>
            </div>
            <ul className="mongo-list">
              {ossBackups.map((item) => (
                <li key={item.key} title={item.key}>
                  <div>
                    <strong className="mono">{item.name}</strong>
                    <span className="dim">
                      {backupKindLabel(item.name)} · {formatBytes(item.size)} · {formatTime(item.lastModified)}
                    </span>
                  </div>
                  <div className="li-actions">
                    <button
                      type="button"
                      className="accent"
                      disabled={Boolean(busy)}
                      onClick={() => void downloadOss(item)}
                    >
                      {busy === `dl:${item.key}` ? '…' : '下载'}
                    </button>
                    <button
                      type="button"
                      className="ghost danger"
                      disabled={Boolean(busy)}
                      onClick={() => void removeOss(item)}
                    >
                      删
                    </button>
                  </div>
                </li>
              ))}
              {!ossBackups.length && (
                <li className="empty">
                  {ossReady ? '暂无对象，点刷新或先去「新建」备份' : '先到「设置」填写 OSS'}
                </li>
              )}
            </ul>
          </section>

          <section className="mongo-card">
            <div className="section-head">
              <div>
                <h3>本机下载</h3>
                <p className="section-sub mono">{localDir || '系统下载目录'}</p>
              </div>
              <div className="section-actions">
                <button type="button" className="ghost" disabled={Boolean(busy)} onClick={() => void pickDir()}>
                  {localDir ? '下载目录' : '选下载目录'}
                </button>
                <button
                  type="button"
                  className="ghost"
                  disabled={Boolean(busy)}
                  onClick={() => void refreshPcBackups()}
                >
                  {busy === 'pcList' ? '…' : '刷新'}
                </button>
              </div>
            </div>
            <ul className="mongo-list">
              {pcBackups.map((item) => (
                <li key={item.path} title={item.path}>
                  <div>
                    <strong className="mono">{item.name}</strong>
                    <span className="dim">
                      {backupKindLabel(item.name)} · {formatBytes(item.size)} · {formatTime(item.mtimeMs)}
                    </span>
                  </div>
                  <div className="li-actions">
                    <button
                      type="button"
                      className="accent"
                      disabled={Boolean(busy)}
                      onClick={() => openRestore(item)}
                    >
                      {busy === `restore:${item.path}` ? '恢复中…' : '恢复到本机'}
                    </button>
                    <button
                      type="button"
                      className="ghost"
                      disabled={Boolean(busy)}
                      onClick={() => void window.api.showItem(item.path)}
                    >
                      打开
                    </button>
                  </div>
                </li>
              ))}
              {!pcBackups.length && (
                <li className="empty">本机暂无 Mongo 备份，可从 OSS 下载或指定目录后刷新</li>
              )}
            </ul>
          </section>

          <section className="mongo-card">
            <div className="section-head">
              <div>
                <h3>服务器备份</h3>
                <p className="section-sub">
                  {cfg.mongoBackupDir || '/var/backups/mongodb'}
                  {cfg.ossInternal ? ' · 上传走服务器内网' : ' · 上传走服务器公网/本机中转'}
                </p>
              </div>
              <button
                type="button"
                className="ghost"
                disabled={Boolean(busy)}
                onClick={() => void refreshLocalBackups()}
              >
                {busy === 'localList' ? '…' : '刷新'}
              </button>
            </div>
            <ul className="mongo-list">
              {localBackups.map((item) => (
                <li key={item.path} title={item.path}>
                  <div>
                    <strong className="mono">{item.name}</strong>
                    <span className="dim">
                      {backupKindLabel(item.name)} · {formatBytes(item.size)} · {formatTime(item.mtimeMs)}
                      {ossNameSet.has(item.name) ? ' · 已在 OSS' : ''}
                    </span>
                  </div>
                  <div className="li-actions">
                    <button
                      type="button"
                      className="accent"
                      disabled={Boolean(busy) || !ossReady}
                      onClick={() => void uploadServerToOss(item)}
                    >
                      {busy === `upOss:${item.path}`
                        ? '上传中…'
                        : ossNameSet.has(item.name)
                          ? '重传 OSS'
                          : '上传 OSS'}
                    </button>
                    <button
                      type="button"
                      className="ghost danger"
                      disabled={Boolean(busy)}
                      onClick={() => void removeLocal(item)}
                    >
                      删
                    </button>
                  </div>
                </li>
              ))}
              {!localBackups.length && <li className="empty">暂无本地备份</li>}
            </ul>
          </section>
        </div>
      )}

      {tab === 'backup' && (
        <div className="mongo-stack">
          <section className="mongo-card">
            <div className="section-head">
              <div>
                <h3>选择数据库</h3>
                <p className="section-sub">
                  全量：库名_bson_时间_mc.zip；增量：库名_bson_incr_时间_mc.zip（需先有全量基线）
                </p>
              </div>
              <div className="section-actions">
                <button
                  type="button"
                  className="ghost"
                  onClick={() => setConnOpen((v) => !v)}
                >
                  {connOpen ? '收起连接' : '连接参数'}
                </button>
                <button
                  type="button"
                  className="primary"
                  disabled={Boolean(busy)}
                  onClick={() => void refreshDbs()}
                >
                  {busy === 'list' ? '加载中…' : '刷新库列表'}
                </button>
              </div>
            </div>

            {connOpen && (
              <div className="mongo-conn-fields">
                <div className="row">
                  <label>
                    主机（SSH 服务器上的 Mongo 地址）
                    <input
                      value={conn.host || ''}
                      onChange={(e) => setConn((c) => ({ ...c, host: e.target.value }))}
                      placeholder="127.0.0.1 = 服务器本机"
                    />
                  </label>
                  <label className="port">
                    端口
                    <input
                      value={String(conn.port ?? 27017)}
                      onChange={(e) =>
                        setConn((c) => ({ ...c, port: Number(e.target.value) || 27017 }))
                      }
                    />
                  </label>
                  <label>
                    认证库
                    <input
                      value={conn.authDb || 'admin'}
                      onChange={(e) => setConn((c) => ({ ...c, authDb: e.target.value }))}
                    />
                  </label>
                </div>
                <div className="row two">
                  <label>
                    用户名
                    <input
                      value={conn.username || ''}
                      onChange={(e) => setConn((c) => ({ ...c, username: e.target.value }))}
                    />
                  </label>
                  <label>
                    密码
                    <input
                      type="password"
                      value={conn.password || ''}
                      onChange={(e) => setConn((c) => ({ ...c, password: e.target.value }))}
                      autoComplete="new-password"
                    />
                  </label>
                </div>
              </div>
            )}

            <ul className="mongo-list">
              {dbs.map((db) => (
                <li key={db.name}>
                  <div>
                    <strong className="mono">{db.name}</strong>
                    <span className="dim">
                      {formatBytes(db.sizeOnDisk)}
                      {db.empty ? ' · 空库' : ''}
                    </span>
                  </div>
                  <div className="li-actions backup-actions">
                    <button
                      type="button"
                      className="accent"
                      disabled={Boolean(busy)}
                      onClick={() => void backup(db.name)}
                    >
                      {busy === `backup:${db.name}` ? '全量中…' : '全量备份'}
                    </button>
                    <button
                      type="button"
                      className="ghost"
                      disabled={Boolean(busy) || !incrReady[db.name]}
                      title={incrReady[db.name] ? '只备份变更集合/文档' : '请先做一次全量备份'}
                      onClick={() => void backupIncremental(db.name)}
                    >
                      {busy === `incr:${db.name}` ? '增量中…' : '增量备份'}
                    </button>
                  </div>
                </li>
              ))}
              {!dbs.length && (
                <li className="empty">点「刷新库列表」加载数据库</li>
              )}
            </ul>
          </section>
        </div>
      )}

      {tab === 'settings' && (
        <div className="mongo-stack">
          <section className="mongo-card">
            <div className="section-head">
              <div>
                <h3>OSS</h3>
                <p className="section-sub">
                  写入 {cfg.configPath || '/etc/default/tuku-backup'}（无 tuku
                  亦可，仅需填凭证）
                </p>
              </div>
            </div>
            <div className="mongo-conn-fields">
              <div className="row two">
                <label>
                  AccessKey ID
                  <input
                    value={cfg.ossAccessKeyId}
                    onChange={(e) => setCfg((c) => ({ ...c, ossAccessKeyId: e.target.value }))}
                  />
                </label>
                <label>
                  AccessKey Secret
                  <input
                    type="password"
                    value={cfg.ossAccessKeySecret}
                    onChange={(e) => setCfg((c) => ({ ...c, ossAccessKeySecret: e.target.value }))}
                    autoComplete="new-password"
                  />
                </label>
              </div>
              <div className="row three">
                <label>
                  Bucket
                  <input
                    value={cfg.ossBucket}
                    onChange={(e) => setCfg((c) => ({ ...c, ossBucket: e.target.value }))}
                  />
                </label>
                <label>
                  Region
                  <input
                    value={cfg.ossRegion}
                    onChange={(e) => setCfg((c) => ({ ...c, ossRegion: e.target.value }))}
                  />
                </label>
                <label>
                  Endpoint
                  <input
                    value={cfg.ossEndpoint}
                    onChange={(e) => setCfg((c) => ({ ...c, ossEndpoint: e.target.value }))}
                  />
                </label>
              </div>
              <div className="row two">
                <label>
                  对象前缀
                  <input
                    value={cfg.ossMongodbPrefix}
                    onChange={(e) => setCfg((c) => ({ ...c, ossMongodbPrefix: e.target.value }))}
                  />
                </label>
                <label>
                  OSS 保留天
                  <input
                    value={String(cfg.ossMongodbRetainDays)}
                    onChange={(e) =>
                      setCfg((c) => ({
                        ...c,
                        ossMongodbRetainDays: Number(e.target.value) || 0
                      }))
                    }
                  />
                </label>
              </div>
              <label className="check">
                <input
                  type="checkbox"
                  checked={cfg.ossInternal}
                  onChange={(e) => setCfg((c) => ({ ...c, ossInternal: e.target.checked }))}
                />
                服务器有 oss-put 时走内网上传（无脚本则本机公网中转，不受此项影响）
              </label>
            </div>
          </section>

          <section className="mongo-card">
            <div className="section-head">
              <div>
                <h3>本地与 Mongo</h3>
                <p className="section-sub">路径、保留策略、可选 URI</p>
              </div>
            </div>
            <div className="mongo-conn-fields">
              <div className="row two">
                <label>
                  可选：tuku-backend 根目录
                  <input
                    value={cfg.tukuBackendRoot}
                    onChange={(e) => setCfg((c) => ({ ...c, tukuBackendRoot: e.target.value }))}
                    placeholder="无 tuku 环境请留空"
                  />
                </label>
                <label>
                  服务器备份目录
                  <input
                    value={cfg.mongoBackupDir}
                    onChange={(e) => setCfg((c) => ({ ...c, mongoBackupDir: e.target.value }))}
                  />
                </label>
              </div>
              <label>
                MONGODB_URL（可选，优先于「新建」里的连接参数）
                <input
                  value={cfg.mongodbUrl}
                  onChange={(e) => setCfg((c) => ({ ...c, mongodbUrl: e.target.value }))}
                  placeholder="mongodb://user:pass@127.0.0.1:27017/tuku"
                />
              </label>
              <label>
                本地保留天
                <input
                  value={String(cfg.mongoBackupRetainDays)}
                  onChange={(e) =>
                    setCfg((c) => ({
                      ...c,
                      mongoBackupRetainDays: Number(e.target.value) || 0
                    }))
                  }
                />
              </label>
            </div>
          </section>

          <div className="mongo-actions sticky-actions">
            <button
              type="button"
              className="primary"
              disabled={Boolean(busy)}
              onClick={() => void saveCfg()}
            >
              {busy === 'saveCfg' ? '保存中…' : '保存到服务器'}
            </button>
            <button
              type="button"
              className="ghost"
              disabled={Boolean(busy)}
              onClick={() =>
                void window.api
                  .loadBackupConfig(sessionId)
                  .then(setCfg)
                  .catch(showErr)
              }
            >
              重新加载
            </button>
            <button type="button" className="ghost" onClick={() => void pickDir()}>
              {localDir ? '更换本机下载目录' : '设置本机下载目录'}
            </button>
          </div>
        </div>
      )}

      {restoreDlg && (
        <div
          className="modal-backdrop"
          role="presentation"
          onClick={() => !busy && setRestoreDlg(null)}
        >
          <div
            className="modal restore-modal"
            role="dialog"
            aria-labelledby="mongo-restore-title"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="modal-head">
              <h2 id="mongo-restore-title">恢复到本机 MongoDB</h2>
              <button
                type="button"
                className="ghost"
                disabled={Boolean(busy)}
                onClick={() => setRestoreDlg(null)}
              >
                取消
              </button>
            </div>
            <div className="modal-body">
              <p className="section-sub mono">{restoreDlg.item.name}</p>
              <p className="dim">写入你电脑上的 MongoDB，不会改远程服务器数据</p>
              {restoreDlg.chainHint && <p className="dim warn">{restoreDlg.chainHint}</p>}
              <div className="row two">
                <label>
                  本机地址
                  <input
                    value={restoreDlg.localConn.host}
                    onChange={(e) =>
                      setRestoreDlg((d) =>
                        d ? { ...d, localConn: { ...d.localConn, host: e.target.value } } : d
                      )
                    }
                    placeholder="127.0.0.1"
                  />
                </label>
                <label className="port">
                  端口
                  <input
                    value={String(restoreDlg.localConn.port)}
                    onChange={(e) =>
                      setRestoreDlg((d) =>
                        d
                          ? {
                              ...d,
                              localConn: {
                                ...d.localConn,
                                port: Number(e.target.value) || 27017
                              }
                            }
                          : d
                      )
                    }
                  />
                </label>
              </div>
              <div className="row two">
                <label>
                  用户名（可空）
                  <input
                    value={restoreDlg.localConn.username || ''}
                    onChange={(e) =>
                      setRestoreDlg((d) =>
                        d ? { ...d, localConn: { ...d.localConn, username: e.target.value } } : d
                      )
                    }
                  />
                </label>
                <label>
                  密码（可空）
                  <input
                    type="password"
                    value={restoreDlg.localConn.password || ''}
                    onChange={(e) =>
                      setRestoreDlg((d) =>
                        d ? { ...d, localConn: { ...d.localConn, password: e.target.value } } : d
                      )
                    }
                    autoComplete="new-password"
                  />
                </label>
              </div>
              <label>
                目标数据库
                <input
                  autoFocus
                  value={restoreDlg.targetDb}
                  onChange={(e) =>
                    setRestoreDlg((d) => (d ? { ...d, targetDb: e.target.value } : d))
                  }
                  placeholder="例如 tuku"
                />
              </label>
              {restoreDlg.sourceHint && restoreDlg.sourceHint !== restoreDlg.targetDb.trim() && (
                <p className="dim">
                  源库名（文件名）：{restoreDlg.sourceHint} → 将映射到目标库
                </p>
              )}
              <div className="auth-switch restore-mode">
                <button
                  type="button"
                  className={restoreDlg.mode === 'merge' ? 'on' : ''}
                  onClick={() => setRestoreDlg((d) => (d ? { ...d, mode: 'merge' } : d))}
                >
                  合并导入
                </button>
                <button
                  type="button"
                  className={restoreDlg.mode === 'replace' ? 'on' : ''}
                  onClick={() => setRestoreDlg((d) => (d ? { ...d, mode: 'replace' } : d))}
                >
                  覆盖（先 drop）
                </button>
              </div>
              {restoreDlg.mode === 'replace' && (
                <p className="form-error">
                  将删除本机库「{restoreDlg.targetDb.trim() || '…'}」后写入
                </p>
              )}
            </div>
            <div className="modal-foot">
              <button type="button" className="primary" onClick={() => void confirmRestore()}>
                开始恢复
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
