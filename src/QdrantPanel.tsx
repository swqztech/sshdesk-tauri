import { useEffect, useMemo, useState } from 'react'
import type { QdrantBackupMetaPeek } from './types'
import type {
  LocalBackupItem,
  OssBackupItem,
  QdrantCollectionInfo,
  QdrantConfig,
  QdrantLocalSettings
} from './types'

type Props = {
  sessionId: string
  active: boolean
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

function parseCollectionFromFileName(fileName: string): string | null {
  const base = fileName.replace(/^.*[/\\]/, '')
  if (/^qdrant_\d{8}_\d{6}\.tar\.gz$/i.test(base)) return null
  const matched = base.match(/^qdrant_(.+)_\d{8}_\d{6}\.tar\.gz$/i)
  return matched?.[1] ?? null
}

const emptyCfg = (): QdrantConfig => ({
  qdrantUrl: 'http://127.0.0.1:6333',
  qdrantApiKey: '',
  qdrantCollection: 'gallery_assets',
  qdrantBackupDir: '/var/backups/qdrant',
  qdrantBackupRetainDays: 7,
  qdrantDataDir: '/var/lib/qdrant',
  qdrantConfigFile: '/etc/qdrant/config.yaml',
  tukuBackendRoot: '',
  ossAccessKeyId: '',
  ossAccessKeySecret: '',
  ossBucket: '',
  ossRegion: 'oss-cn-shenzhen',
  ossEndpoint: 'oss-cn-shenzhen.aliyuncs.com',
  ossInternal: true,
  ossBackupRoot: 'backups',
  ossQdrantPrefix: 'backups/qdrant',
  ossQdrantRetainDays: 14,
  configPath: '/etc/default/qdrant'
})

export default function QdrantPanel({ sessionId, active }: Props) {
  const [tab, setTab] = useState<Tab>('files')
  const [cfg, setCfg] = useState<QdrantConfig>(emptyCfg)
  const [collections, setCollections] = useState<QdrantCollectionInfo[]>([])
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
  const [restoreDlg, setRestoreDlg] = useState<LocalBackupItem | null>(null)
  const [restoreMeta, setRestoreMeta] = useState<QdrantBackupMetaPeek | null>(null)
  const [localRestore, setLocalRestore] = useState<QdrantLocalSettings>({
    qdrantExe: '',
    qdrantDataDir: ''
  })
  const [localStatus, setLocalStatus] = useState<string>('')

  const ossReady = Boolean(cfg.ossAccessKeyId && cfg.ossAccessKeySecret && cfg.ossBucket)
  const ossNameSet = useMemo(() => new Set(ossBackups.map((o) => o.name)), [ossBackups])
  const statusLine = useMemo(() => {
    return [
      cfg.qdrantUrl || '未设置 URL',
      ossReady ? `OSS · ${cfg.ossBucket}` : 'OSS 未配置',
      localDir ? '已选下载目录' : '下载时再选目录'
    ].join('  ·  ')
  }, [cfg.qdrantUrl, ossReady, cfg.ossBucket, localDir])

  useEffect(() => {
    return window.api.onQdrantBackupProgress((p) => {
      if (p.sessionId !== sessionId) return
      setProgress({ message: p.message, percent: p.percent, label: p.collection })
    })
  }, [sessionId])

  useEffect(() => {
    if (!active) return
    void (async () => {
      setBusy('boot')
      setToast(null)
      try {
        const loaded = await window.api.loadQdrantConfig(sessionId)
        setCfg(loaded)
        const [local, oss] = await Promise.all([
          window.api.listLocalQdrantBackups(sessionId, loaded),
          loaded.ossAccessKeyId && loaded.ossAccessKeySecret && loaded.ossBucket
            ? window.api.listOssQdrantBackups(loaded)
            : Promise.resolve([] as OssBackupItem[])
        ])
        setLocalBackups(local)
        setOssBackups(oss)
        const dl = await window.api.defaultDownloadDir()
        setLocalDir((prev) => prev || dl)
        setPcBackups(await window.api.listPcQdrantBackups(dl))
        const localSet = await window.api.loadQdrantLocalSettings()
        if (!localSet.qdrantExe) {
          const detected = await window.api.detectLocalQdrantExe()
          if (detected) {
            const saved = await window.api.saveQdrantLocalSettings({
              ...localSet,
              qdrantExe: detected
            })
            setLocalRestore(saved)
          } else {
            setLocalRestore(localSet)
          }
        } else {
          setLocalRestore(localSet)
        }
        try {
          const st = await window.api.probeLocalQdrantStatus(loaded.qdrantCollection)
          setLocalStatus(st.message)
        } catch {
          setLocalStatus('')
        }
      } catch (err) {
        const fallback = await window.api.defaultQdrantConfig()
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

  async function refreshCollections() {
    setBusy('list')
    setToast(null)
    try {
      setCollections(await window.api.listQdrantCollections(sessionId, cfg))
    } catch (err) {
      showErr(err)
      setCollections([])
    } finally {
      setBusy('')
    }
  }

  async function refreshOss(nextCfg = cfg) {
    setBusy('ossList')
    setToast(null)
    try {
      if (!nextCfg.ossAccessKeyId || !nextCfg.ossAccessKeySecret || !nextCfg.ossBucket) {
        setOssBackups([])
        setToast({ kind: 'err', text: '请先在「设置」里填写 OSS' })
        return
      }
      setOssBackups(await window.api.listOssQdrantBackups(nextCfg))
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
      setPcBackups(await window.api.listPcQdrantBackups(root))
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function refreshLocal(nextCfg = cfg) {
    setBusy('localList')
    setToast(null)
    try {
      setLocalBackups(await window.api.listLocalQdrantBackups(sessionId, nextCfg))
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function refreshAll(nextCfg = cfg) {
    setBusy('backups')
    setToast(null)
    try {
      const [local, oss] = await Promise.all([
        window.api.listLocalQdrantBackups(sessionId, nextCfg),
        nextCfg.ossAccessKeyId && nextCfg.ossAccessKeySecret && nextCfg.ossBucket
          ? window.api.listOssQdrantBackups(nextCfg)
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
      const saved = await window.api.saveQdrantConfig(sessionId, cfg)
      setCfg(saved)
      showOk(`配置已保存到 ${saved.configPath}`)
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function backup(coll: string) {
    setBusy(`backup:${coll}`)
    setToast(null)
    setProgress({ message: '准备备份…', percent: 2, label: coll })
    try {
      const res = await window.api.backupQdrant(sessionId, coll, cfg)
      setProgress({
        message: res.uploaded ? '已上传 OSS' : '仅保留在服务器',
        percent: 100,
        label: coll
      })
      showOk(
        res.uploaded ? `${res.fileName} 已上传 OSS` : `${res.fileName} 已写到服务器（未上传 OSS）`
      )
      await refreshAll()
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
      const res = await window.api.downloadOssQdrant(sessionId, cfg, item.key, dir)
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
      await window.api.deleteLocalQdrantBackup(sessionId, item.path)
      await refreshLocal()
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
      await window.api.deleteOssQdrantBackup(cfg, item.key)
      await refreshOss()
      showOk(`已删除 ${item.name}`)
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function saveLocalRestoreSettings() {
    setBusy('saveLocal')
    setToast(null)
    try {
      const saved = await window.api.saveQdrantLocalSettings(localRestore)
      setLocalRestore(saved)
      showOk('本机恢复配置已保存')
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function detectLocalExe() {
    setBusy('detectExe')
    setToast(null)
    try {
      const p = await window.api.detectLocalQdrantExe()
      if (!p) {
        setToast({ kind: 'err', text: '未探测到 qdrant.exe，请手动选择或先启动 Qdrant' })
        return
      }
      setLocalRestore((s) => ({ ...s, qdrantExe: p }))
      showOk(`已探测：${p}`)
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function openRestore(item: LocalBackupItem) {
    setToast(null)
    setRestoreDlg(item)
    setRestoreMeta(null)
    try {
      const meta = await window.api.peekQdrantBackupMeta(item.path)
      setRestoreMeta(meta)
    } catch {
      setRestoreMeta(null)
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
      const res = await window.api.uploadServerQdrantToOss(sessionId, item.path, cfg)
      const viaLabel =
        res.via === 'server-internal'
          ? '服务器内网'
          : res.via === 'server-public'
            ? '服务器公网'
            : '本机中转'
      setProgress({ message: '上传完成', percent: 100, label: item.name })
      showOk(`${item.name} 已上传 OSS（${viaLabel}）`)
      await refreshOss()
    } catch (err) {
      showErr(err)
      setProgress(null)
    } finally {
      setBusy('')
    }
  }

  async function confirmRestore() {
    if (!restoreDlg) return
    const item = restoreDlg
    const meta = restoreMeta
    setRestoreDlg(null)
    setRestoreMeta(null)
    setBusy(`restore:${item.path}`)
    setToast(null)
    const targetCollection = meta?.collection?.trim() || cfg.qdrantCollection || 'gallery_assets'
    setProgress({ message: '准备恢复…', percent: 2, label: targetCollection })
    try {
      await window.api.saveQdrantLocalSettings(localRestore)
      const res = await window.api.restoreQdrantFromLocal(
        sessionId,
        item.path,
        cfg,
        meta?.collection
      )
      setProgress({ message: '恢复完成', percent: 100, label: res.collection })
      if (res.collection && res.collection !== cfg.qdrantCollection) {
        const nextCfg = { ...cfg, qdrantCollection: res.collection }
        await window.api.saveQdrantConfig(sessionId, nextCfg)
        setCfg(nextCfg)
      }
      showOk(
        `已恢复并确认持久化：${res.collection}（${res.pointsCount.toLocaleString()} points）。之后请用「启动本机 Qdrant」重启`,
        res.dataDir
      )
      try {
        const st = await window.api.probeLocalQdrantStatus(res.collection)
        setLocalStatus(st.message)
      } catch {
        // ignore
      }
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

  async function startLocalQdrant() {
    setBusy('startLocal')
    setToast(null)
    try {
      await window.api.saveQdrantLocalSettings(localRestore)
      const st = await window.api.startLocalQdrant(cfg.qdrantCollection)
      setLocalStatus(st.message)
      showOk(st.message, st.dataDir)
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function stopLocalQdrant() {
    setBusy('stopLocal')
    setToast(null)
    try {
      const st = await window.api.stopLocalQdrant()
      setLocalStatus(st.message)
      showOk(st.message)
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  async function refreshLocalStatus() {
    setBusy('probeLocal')
    setToast(null)
    try {
      const st = await window.api.probeLocalQdrantStatus(cfg.qdrantCollection)
      setLocalStatus(st.message)
      showOk(st.message, st.dataDir)
    } catch (err) {
      showErr(err)
    } finally {
      setBusy('')
    }
  }

  return (
    <div className="mongo-panel">
      <header className="mongo-top">
        <div className="mongo-top-main">
          <h2>Qdrant</h2>
          <p className="mongo-status">{busy === 'boot' ? '加载配置中…' : statusLine}</p>
        </div>
        <nav className="mongo-tabs" aria-label="Qdrant 分区">
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
                <p className="section-sub">qdrant_集合名_YYYYMMDD_HHMMSS.tar.gz，推荐从此下载</p>
              </div>
              <div className="section-actions">
                <button type="button" className="ghost" disabled={Boolean(busy)} onClick={() => void pickDir()}>
                  {localDir ? '下载目录' : '选下载目录'}
                </button>
                <button
                  type="button"
                  className="ghost"
                  disabled={Boolean(busy)}
                  onClick={() => void refreshOss()}
                >
                  {busy === 'ossList' ? '…' : '刷新'}
                </button>
              </div>
            </div>
            <ul className="mongo-list">
              {ossBackups.map((item) => {
                const collHint = parseCollectionFromFileName(item.name)
                return (
                <li key={item.key} title={item.key}>
                  <div>
                    <strong className="mono">{item.name}</strong>
                    <span className="dim">
                      {formatBytes(item.size)} · {formatTime(item.lastModified)}
                      {collHint ? ` · 集合 ${collHint}` : ''}
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
              )})}
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
                {localStatus && <p className="section-sub">{localStatus}</p>}
              </div>
              <div className="section-actions">
                <button
                  type="button"
                  className="accent"
                  disabled={Boolean(busy)}
                  onClick={() => void startLocalQdrant()}
                >
                  {busy === 'startLocal' ? '启动中…' : '启动本机 Qdrant'}
                </button>
                <button
                  type="button"
                  className="ghost"
                  disabled={Boolean(busy)}
                  onClick={() => void stopLocalQdrant()}
                >
                  {busy === 'stopLocal' ? '…' : '停止'}
                </button>
                <button
                  type="button"
                  className="ghost"
                  disabled={Boolean(busy)}
                  onClick={() => void refreshLocalStatus()}
                >
                  {busy === 'probeLocal' ? '…' : '状态'}
                </button>
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
              {pcBackups.map((item) => {
                const collHint = parseCollectionFromFileName(item.name)
                return (
                <li key={item.path} title={item.path}>
                  <div>
                    <strong className="mono">{item.name}</strong>
                    <span className="dim">
                      {formatBytes(item.size)} · {formatTime(item.mtimeMs)}
                      {collHint ? ` · 集合 ${collHint}` : ''}
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
              )})}
              {!pcBackups.length && (
                <li className="empty">本机暂无 Qdrant 备份，可从 OSS 下载或指定目录后刷新</li>
              )}
            </ul>
          </section>

          <section className="mongo-card">
            <div className="section-head">
              <div>
                <h3>服务器备份</h3>
                <p className="section-sub">
                  {cfg.qdrantBackupDir || '/var/backups/qdrant'}
                  {cfg.ossInternal ? ' · 上传走服务器内网' : ' · 上传走服务器公网/本机中转'}
                </p>
              </div>
              <button
                type="button"
                className="ghost"
                disabled={Boolean(busy)}
                onClick={() => void refreshLocal()}
              >
                {busy === 'localList' ? '…' : '刷新'}
              </button>
            </div>
            <ul className="mongo-list">
              {localBackups.map((item) => {
                const collHint = parseCollectionFromFileName(item.name)
                return (
                <li key={item.path} title={item.path}>
                  <div>
                    <strong className="mono">{item.name}</strong>
                    <span className="dim">
                      {formatBytes(item.size)} · {formatTime(item.mtimeMs)}
                      {collHint ? ` · 集合 ${collHint}` : ''}
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
              )})}
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
                <h3>选择集合</h3>
                <p className="section-sub">
                  创建集合快照 → 只打包本次 .snapshot → qdrant_集合名_时间.tar.gz → OSS（不做整份 storage）
                </p>
              </div>
              <button
                type="button"
                className="primary"
                disabled={Boolean(busy)}
                onClick={() => void refreshCollections()}
              >
                {busy === 'list' ? '加载中…' : '刷新集合'}
              </button>
            </div>
            <ul className="mongo-list">
              {collections.map((c) => (
                <li key={c.name}>
                  <div>
                    <strong className="mono">{c.name}</strong>
                    <span className="dim">
                      {c.pointsCount.toLocaleString()} points · {c.status}
                    </span>
                  </div>
                  <button
                    type="button"
                    className="accent"
                    disabled={Boolean(busy)}
                    onClick={() => void backup(c.name)}
                  >
                    {busy === `backup:${c.name}` ? '备份中…' : '备份并上传'}
                  </button>
                </li>
              ))}
              {!collections.length && (
                <li className="empty">点「刷新集合」加载（默认库 {cfg.qdrantCollection || '—'}）</li>
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
                <h3>本机恢复</h3>
                <p className="section-sub">
                  与图库开发环境统一：C:\qdrant\qdrant.exe + C:\qdrant\storage（Qdrant 1.19）
                </p>
                {localStatus && <p className="section-sub">{localStatus}</p>}
              </div>
            </div>
            <div className="mongo-conn-fields">
              <label>
                qdrant.exe 路径
                <div className="row">
                  <input
                    className="grow mono"
                    value={localRestore.qdrantExe}
                    onChange={(e) =>
                      setLocalRestore((s) => ({ ...s, qdrantExe: e.target.value }))
                    }
                    placeholder="C:\qdrant\qdrant.exe（与 start-tuku-dev.bat 一致）"
                  />
                  <button
                    type="button"
                    className="ghost"
                    disabled={Boolean(busy)}
                    onClick={() => void detectLocalExe()}
                  >
                    {busy === 'detectExe' ? '…' : '自动探测'}
                  </button>
                  <button
                    type="button"
                    className="ghost"
                    disabled={Boolean(busy)}
                    onClick={() => {
                      void window.api.pickQdrantExe().then((p) => {
                        if (p) setLocalRestore((s) => ({ ...s, qdrantExe: p }))
                      })
                    }}
                  >
                    浏览
                  </button>
                </div>
              </label>
              <label>
                本机数据目录
                <input
                  className="mono"
                  value={localRestore.qdrantDataDir}
                  onChange={(e) =>
                    setLocalRestore((s) => ({ ...s, qdrantDataDir: e.target.value }))
                  }
                  placeholder="C:\qdrant"
                />
              </label>
            </div>
            <div className="mongo-actions">
              <button
                type="button"
                className="primary"
                disabled={Boolean(busy)}
                onClick={() => void saveLocalRestoreSettings()}
              >
                {busy === 'saveLocal' ? '保存中…' : '保存本机配置'}
              </button>
              <button
                type="button"
                className="accent"
                disabled={Boolean(busy)}
                onClick={() => void startLocalQdrant()}
              >
                {busy === 'startLocal' ? '启动中…' : '启动本机 Qdrant'}
              </button>
              <button
                type="button"
                className="ghost"
                disabled={Boolean(busy)}
                onClick={() => void stopLocalQdrant()}
              >
                {busy === 'stopLocal' ? '…' : '停止本机'}
              </button>
            </div>
          </section>

          <section className="mongo-card">
            <div className="section-head">
              <div>
                <h3>Qdrant</h3>
                <p className="section-sub">写入 {cfg.configPath || '/etc/default/qdrant'}</p>
              </div>
            </div>
            <div className="mongo-conn-fields">
              <div className="row two">
                <label>
                  URL
                  <input
                    value={cfg.qdrantUrl}
                    onChange={(e) => setCfg((c) => ({ ...c, qdrantUrl: e.target.value }))}
                  />
                </label>
                <label>
                  API Key（可空）
                  <input
                    type="password"
                    value={cfg.qdrantApiKey}
                    onChange={(e) => setCfg((c) => ({ ...c, qdrantApiKey: e.target.value }))}
                    autoComplete="new-password"
                  />
                </label>
              </div>
              <div className="row two">
                <label>
                  默认集合
                  <input
                    value={cfg.qdrantCollection}
                    onChange={(e) => setCfg((c) => ({ ...c, qdrantCollection: e.target.value }))}
                  />
                </label>
                <label>
                  本地备份目录
                  <input
                    value={cfg.qdrantBackupDir}
                    onChange={(e) => setCfg((c) => ({ ...c, qdrantBackupDir: e.target.value }))}
                  />
                </label>
              </div>
              <div className="row two">
                <label>
                  数据目录
                  <input
                    value={cfg.qdrantDataDir}
                    onChange={(e) => setCfg((c) => ({ ...c, qdrantDataDir: e.target.value }))}
                  />
                </label>
                <label>
                  配置文件
                  <input
                    value={cfg.qdrantConfigFile}
                    onChange={(e) => setCfg((c) => ({ ...c, qdrantConfigFile: e.target.value }))}
                  />
                </label>
              </div>
              <label>
                本地保留天
                <input
                  value={String(cfg.qdrantBackupRetainDays)}
                  onChange={(e) =>
                    setCfg((c) => ({
                      ...c,
                      qdrantBackupRetainDays: Number(e.target.value) || 0
                    }))
                  }
                />
              </label>
            </div>
          </section>

          <section className="mongo-card">
            <div className="section-head">
              <div>
                <h3>OSS</h3>
                <p className="section-sub">无 tuku 亦可，填齐凭证即可备份上传</p>
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
                    value={cfg.ossQdrantPrefix}
                    onChange={(e) => setCfg((c) => ({ ...c, ossQdrantPrefix: e.target.value }))}
                  />
                </label>
                <label>
                  OSS 保留天
                  <input
                    value={String(cfg.ossQdrantRetainDays)}
                    onChange={(e) =>
                      setCfg((c) => ({
                        ...c,
                        ossQdrantRetainDays: Number(e.target.value) || 0
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
                服务器有 oss-put 时走内网上传（无脚本则本机公网中转）
              </label>
              <label>
                可选：tuku-backend 根目录（无则留空）
                <input
                  value={cfg.tukuBackendRoot}
                  onChange={(e) => setCfg((c) => ({ ...c, tukuBackendRoot: e.target.value }))}
                  placeholder="无 tuku 环境请留空"
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
                void window.api.loadQdrantConfig(sessionId).then(setCfg).catch(showErr)
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
          onClick={() => {
            if (busy) return
            setRestoreDlg(null)
            setRestoreMeta(null)
          }}
        >
          <div
            className="modal restore-modal"
            role="dialog"
            aria-labelledby="qdrant-restore-title"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="modal-head">
              <h2 id="qdrant-restore-title">恢复到本机 Qdrant</h2>
              <button
                type="button"
                className="ghost"
                disabled={Boolean(busy)}
                onClick={() => {
                  setRestoreDlg(null)
                  setRestoreMeta(null)
                }}
              >
                取消
              </button>
            </div>
            <div className="modal-body">
              <p className="section-sub mono">{restoreDlg.name}</p>
              <p className="dim">
                将解压并加载到 {localRestore.qdrantDataDir || 'C:\\qdrant'}
              </p>
              <p className="dim">
                备份集合：
                <strong>
                  {restoreMeta === null
                    ? '读取中…'
                    : restoreMeta.collection ||
                      `${cfg.qdrantCollection || 'gallery_assets'}（备份无 meta，将用配置兜底）`}
                </strong>
                {restoreMeta?.collection &&
                  cfg.qdrantCollection &&
                  restoreMeta.collection !== cfg.qdrantCollection &&
                  `（当前配置为 ${cfg.qdrantCollection}，恢复后将自动更新）`}
              </p>
              <p className="dim">
                仅恢复上方集合，本机其它集合会保留；若同名集合已存在会先删除再写入。
              </p>
              <p className="dim mono">
                qdrant.exe：{localRestore.qdrantExe || '未配置（将自动探测运行中的进程）'}
              </p>
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
