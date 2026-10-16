import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { FileBrowserState, FileBrowserTab, RemoteFileEntry } from './types'

type Props = {
  sessionId: string
  hostId: string
  active: boolean
  onOpenTerminal: (remotePath: string) => void
}

type FileClipboardItem = {
  path: string
  name: string
  isDirectory: boolean
}

type FileClipboard = {
  mode: 'copy' | 'cut'
  sessionId: string
  items: FileClipboardItem[]
}

type ContextMenuState = {
  x: number
  y: number
  item: RemoteFileEntry | null
}

function formatBytes(n: number) {
  if (!Number.isFinite(n) || n < 0) return '—'
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(2)} MB`
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`
}

function formatTime(ms: number) {
  if (!ms) return '—'
  return new Date(ms).toLocaleString()
}

function newTabId() {
  return `t-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`
}

function toClipboardItem(entry: RemoteFileEntry): FileClipboardItem {
  return { path: entry.path, name: entry.name, isDirectory: entry.isDirectory }
}

export default function FileManagerPanel({ sessionId, hostId, active, onOpenTerminal }: Props) {
  const panelRef = useRef<HTMLDivElement>(null)
  const [browser, setBrowser] = useState<FileBrowserState | null>(null)
  const [entries, setEntries] = useState<RemoteFileEntry[]>([])
  const [currentPath, setCurrentPath] = useState('/')
  const [parent, setParent] = useState<string | null>(null)
  const [busy, setBusy] = useState('')
  const [error, setError] = useState('')
  const [currentDirSize, setCurrentDirSize] = useState<{ human: string; bytes: number } | null>(
    null
  )
  const [dirSizes, setDirSizes] = useState<Record<string, string>>({})
  const [sizeBusyPath, setSizeBusyPath] = useState('')
  const [downloadBusyPath, setDownloadBusyPath] = useState('')
  const [downloadProgress, setDownloadProgress] = useState<{
    remotePath: string
    percent: number
    label: string
  } | null>(null)
  const [toast, setToast] = useState<{ kind: 'ok' | 'err'; text: string; path?: string } | null>(
    null
  )
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(new Set())
  const [clipboard, setClipboard] = useState<FileClipboard | null>(null)
  const [ctxMenu, setCtxMenu] = useState<ContextMenuState | null>(null)

  const activeTab = useMemo(
    () => browser?.tabs.find((t) => t.id === browser.activeTabId),
    [browser]
  )

  const selectedEntries = useMemo(
    () => entries.filter((e) => selectedPaths.has(e.path)),
    [entries, selectedPaths]
  )

  const persist = useCallback(
    (next: FileBrowserState) => {
      setBrowser(next)
      void window.api.saveFileBrowserState(hostId, next)
    },
    [hostId]
  )

  const loadDir = useCallback(
    async (path: string, tabId?: string) => {
      setBusy('list')
      setError('')
      try {
        const res = await window.api.listRemoteDir(sessionId, path)
        setCurrentPath(res.path)
        setParent(res.parent)
        setEntries(res.entries)
        setSelectedPaths(new Set())
        setCurrentDirSize(null)
        setDirSizes({})
        if (tabId) {
          setBrowser((prev) => {
            if (!prev) return prev
            const tabs = prev.tabs.map((t) =>
              t.id === tabId ? { ...t, path: res.path } : t
            )
            const next = { tabs, activeTabId: tabId }
            void window.api.saveFileBrowserState(hostId, next)
            return next
          })
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
        setEntries([])
      } finally {
        setBusy('')

      }
    },
    [sessionId, hostId]
  )

  useEffect(() => {
    const off = window.api.onFileDownloadProgress((p) => {
      if (p.sessionId !== sessionId) return
      const percent =
        p.total > 0 ? Math.min(99, Math.round((p.transferred / p.total) * 100)) : 0
      setDownloadProgress({
        remotePath: p.remotePath,
        percent,
        label: p.remotePath.split('/').pop() || p.remotePath
      })
    })
    return off
  }, [sessionId])

  useEffect(() => {
    if (!active) return
    void (async () => {
      setBusy('boot')
      setError('')
      try {
        const st = await window.api.loadFileBrowserState(hostId, sessionId)
        setBrowser(st)
        const tab = st.tabs.find((t) => t.id === st.activeTabId) || st.tabs[0]
        if (tab) await loadDir(tab.path, tab.id)
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
      } finally {
        setBusy('')
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId, hostId, active])

  useEffect(() => {
    if (!ctxMenu) return
    const close = () => setCtxMenu(null)
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close()
    }
    document.addEventListener('click', close)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('click', close)
      document.removeEventListener('keydown', onKey)
    }
  }, [ctxMenu])

  useEffect(() => {
    if (active) panelRef.current?.focus()
  }, [active])

  if (!active) return null

  function selectTab(tab: FileBrowserTab) {
    if (!browser) return
    persist({ ...browser, activeTabId: tab.id })
    void loadDir(tab.path, tab.id)
  }

  function addTab(atPath?: string) {
    if (!browser) return
    const path = atPath || currentPath || activeTab?.path || '/'
    const tab: FileBrowserTab = { id: newTabId(), path }
    const tabs = [...browser.tabs, tab]
    persist({ tabs, activeTabId: tab.id })
    void loadDir(path, tab.id)
  }

  function closeTab(tabId: string) {
    if (!browser || browser.tabs.length <= 1) return
    const tabs = browser.tabs.filter((t) => t.id !== tabId)
    const activeTabId =
      browser.activeTabId === tabId ? tabs[tabs.length - 1]!.id : browser.activeTabId
    persist({ tabs, activeTabId })
    const next = tabs.find((t) => t.id === activeTabId)
    if (next) void loadDir(next.path, next.id)
  }

  function enterDir(entry: RemoteFileEntry) {
    if (!entry.isDirectory || !browser) return
    const tabId = browser.activeTabId
    void loadDir(entry.path, tabId)
  }

  function goParent() {
    if (!parent || !browser) return
    void loadDir(parent, browser.activeTabId)
  }

  function goPath() {
    const p = window.prompt('输入远程路径', currentPath)
    if (!p?.trim() || !browser) return
    void loadDir(p.trim(), browser.activeTabId)
  }

  function openTerminalHere() {
    onOpenTerminal(currentPath)
  }

  function selectEntry(entry: RemoteFileEntry, multi: boolean) {
    setSelectedPaths((prev) => {
      if (multi) {
        const next = new Set(prev)
        if (next.has(entry.path)) next.delete(entry.path)
        else next.add(entry.path)
        return next
      }
      return new Set([entry.path])
    })
  }

  function openContextMenu(e: React.MouseEvent, item: RemoteFileEntry | null) {
    e.preventDefault()
    e.stopPropagation()
    if (item) selectEntry(item, e.ctrlKey || e.metaKey)
    setCtxMenu({ x: e.clientX, y: e.clientY, item })
  }

  function copyItems(items: FileClipboardItem[]) {
    if (!items.length) return
    setClipboard({ mode: 'copy', sessionId, items })
    setToast({ kind: 'ok', text: `已复制 ${items.length} 项` })
  }

  function cutItems(items: FileClipboardItem[]) {
    if (!items.length) return
    setClipboard({ mode: 'cut', sessionId, items })
    setToast({ kind: 'ok', text: `已剪切 ${items.length} 项` })
  }

  async function copyPath(path: string) {
    try {
      await navigator.clipboard.writeText(path)
      setToast({ kind: 'ok', text: '已复制路径到剪贴板' })
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  async function pasteClipboard() {
    if (!clipboard?.items.length || clipboard.sessionId !== sessionId || !browser) return
    setBusy('paste')
    setError('')
    setToast(null)
    const sources = clipboard.items.map((i) => i.path)
    try {
      const res =
        clipboard.mode === 'copy'
          ? await window.api.copyRemotePaths(sessionId, sources, currentPath)
          : await window.api.moveRemotePaths(sessionId, sources, currentPath)
      const verb = clipboard.mode === 'copy' ? '复制' : '移动'
      setToast({ kind: 'ok', text: `已${verb} ${res.count} 项到当前目录` })
      if (clipboard.mode === 'cut') setClipboard(null)
      await loadDir(currentPath, browser.activeTabId)
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setError(msg)
      setToast({ kind: 'err', text: msg })
    } finally {
      setBusy('')
    }
  }

  function copySelected() {
    copyItems(selectedEntries.map(toClipboardItem))
  }

  function cutSelected() {
    cutItems(selectedEntries.map(toClipboardItem))
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    const mod = e.ctrlKey || e.metaKey
    if (!mod) return
    const key = e.key.toLowerCase()
    if (key === 'c') {
      e.preventDefault()
      if (selectedEntries.length) copySelected()
    } else if (key === 'x') {
      e.preventDefault()
      if (selectedEntries.length) cutSelected()
    } else if (key === 'v') {
      e.preventDefault()
      void pasteClipboard()
    }
  }

  async function downloadFile(item: RemoteFileEntry) {
    if (item.isDirectory) return
    setDownloadBusyPath(item.path)
    setDownloadProgress({ remotePath: item.path, percent: 0, label: item.name })
    setError('')
    setToast(null)
    try {
      const res = await window.api.downloadRemoteFile(sessionId, item.path)
      setDownloadProgress({ remotePath: item.path, percent: 100, label: item.name })
      setToast({
        kind: 'ok',
        text: `已下载 ${item.name}（${formatBytes(res.bytes)}）`,
        path: res.localPath
      })
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      if (msg !== '已取消下载') {
        setError(msg)
        setToast({ kind: 'err', text: msg })
      }
      setDownloadProgress(null)
    } finally {
      setDownloadBusyPath('')
      setTimeout(() => setDownloadProgress(null), 1200)
    }
  }

  async function statDir(path: string, key = path) {
    setSizeBusyPath(key)
    setError('')
    try {
      const res = await window.api.statRemoteDirSize(sessionId, path)
      if (path === currentPath) {
        setCurrentDirSize({ human: res.human, bytes: res.bytes })
      }
      setDirSizes((prev) => ({ ...prev, [path]: res.human }))
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSizeBusyPath('')
    }
  }

  const crumbs = currentPath === '/' ? ['/'] : currentPath.split('/').filter(Boolean)
  const canPaste = Boolean(clipboard?.items.length && clipboard.sessionId === sessionId)
  const ctxTarget = ctxMenu?.item
  const ctxSelected =
    ctxTarget && selectedPaths.has(ctxTarget.path)
      ? selectedEntries
      : ctxTarget
        ? [ctxTarget]
        : selectedEntries

  return (
    <div
      ref={panelRef}
      className="mongo-panel file-panel"
      tabIndex={0}
      onKeyDown={handleKeyDown}
    >
      <header className="mongo-top">
        <div className="mongo-top-main">
          <h2>文件管理</h2>
          <p className="mongo-status mono">
            {currentPath || '—'}
            {currentDirSize ? (
              <span className="file-dir-size"> · 共 {currentDirSize.human}</span>
            ) : null}
            {clipboard?.items.length ? (
              <span className="file-clipboard-hint">
                {' '}
                · 剪贴板：{clipboard.mode === 'copy' ? '复制' : '剪切'} {clipboard.items.length} 项
              </span>
            ) : null}
          </p>
        </div>
        <div className="section-actions">
          <button
            type="button"
            className="ghost"
            disabled={Boolean(busy) || !selectedEntries.length}
            onClick={copySelected}
          >
            复制
          </button>
          <button
            type="button"
            className="ghost"
            disabled={Boolean(busy) || !selectedEntries.length}
            onClick={cutSelected}
          >
            剪切
          </button>
          <button
            type="button"
            className="ghost"
            disabled={Boolean(busy) || !canPaste}
            onClick={() => void pasteClipboard()}
          >
            粘贴
          </button>
          <button
            type="button"
            className="ghost"
            disabled={Boolean(busy) || sizeBusyPath === currentPath}
            onClick={() => void statDir(currentPath)}
          >
            {sizeBusyPath === currentPath ? '统计中…' : '统计本目录'}
          </button>
          <button type="button" className="accent" disabled={Boolean(busy)} onClick={openTerminalHere}>
            新终端打开
          </button>
          <button type="button" className="ghost" disabled={Boolean(busy)} onClick={() => addTab()}>
            新标签
          </button>
          <button
            type="button"
            className="ghost"
            disabled={Boolean(busy) || !parent}
            onClick={goParent}
          >
            上级
          </button>
          <button type="button" className="ghost" disabled={Boolean(busy)} onClick={goPath}>
            跳转
          </button>
          <button
            type="button"
            className="ghost"
            disabled={Boolean(busy)}
            onClick={() => activeTab && void loadDir(activeTab.path, activeTab.id)}
          >
            {busy === 'list' ? '…' : '刷新'}
          </button>
        </div>
      </header>

      <div className="file-tab-bar">
        {browser?.tabs.map((tab) => {
          const label = tab.path.length > 28 ? `…${tab.path.slice(-26)}` : tab.path
          return (
            <div key={tab.id} className={`file-tab ${tab.id === browser.activeTabId ? 'on' : ''}`}>
              <button type="button" className="file-tab-btn" title={tab.path} onClick={() => selectTab(tab)}>
                {label}
              </button>
              {browser.tabs.length > 1 && (
                <button
                  type="button"
                  className="file-tab-x"
                  aria-label="关闭目录标签"
                  onClick={() => closeTab(tab.id)}
                >
                  ×
                </button>
              )}
            </div>
          )
        })}
      </div>

      <nav className="file-breadcrumb" aria-label="路径">
        <button type="button" className="ghost crumb" disabled={!parent} onClick={goParent}>
          /
        </button>
        {crumbs[0] === '/' ? null : (
          <>
            {crumbs.map((part, i) => {
              const full = '/' + crumbs.slice(0, i + 1).join('/')
              return (
                <span key={full} className="crumb-wrap">
                  <span className="crumb-sep">/</span>
                  <button
                    type="button"
                    className="ghost crumb"
                    onClick={() => browser && void loadDir(full, browser.activeTabId)}
                  >
                    {part}
                  </button>
                </span>
              )
            })}
          </>
        )}
      </nav>

      {toast && (
        <div className={`mongo-toast ${toast.kind === 'ok' ? 'ok' : 'err'}`}>
          <span>{toast.text}</span>
          {toast.path && toast.kind === 'ok' && (
            <button type="button" className="ghost" onClick={() => void window.api.showItem(toast.path!)}>
              在文件夹中显示
            </button>
          )}
        </div>
      )}
      {error && <div className="banner error file-banner">{error}</div>}

      <section
        className="mongo-card file-list-card"
        onContextMenu={(e) => openContextMenu(e, null)}
      >
        <ul className="mongo-list file-list">
          {parent && (
            <li className="file-item">
              <button type="button" className="file-item-main" onClick={goParent}>
                <span className="file-icon">📁</span>
                <div>
                  <strong>..</strong>
                  <span className="dim">上级目录</span>
                </div>
              </button>
            </li>
          )}
          {entries.map((item) => {
            const downloading = downloadBusyPath === item.path
            const percent =
              downloading && downloadProgress?.remotePath === item.path
                ? downloadProgress.percent
                : 0
            const selected = selectedPaths.has(item.path)
            return (
              <li
                key={item.path}
                className={`file-item${selected ? ' selected' : ''}`}
                onContextMenu={(e) => openContextMenu(e, item)}
              >
                {item.isDirectory ? (
                  <button
                    type="button"
                    className="file-item-main"
                    onClick={(e) => selectEntry(item, e.ctrlKey || e.metaKey)}
                    onDoubleClick={() => enterDir(item)}
                  >
                    <span className="file-icon">📁</span>
                    <div>
                      <strong className="mono">{item.name}</strong>
                      <span className="dim">
                        {dirSizes[item.path] ? `目录 · ${dirSizes[item.path]}` : '目录'}
                      </span>
                    </div>
                  </button>
                ) : (
                  <div
                    className="file-item-main file-item-main--static"
                    onClick={(e) => selectEntry(item, e.ctrlKey || e.metaKey)}
                  >
                    <span className="file-icon">📄</span>
                    <div>
                      <strong className="mono">{item.name}</strong>
                      <span className="dim">
                        {formatBytes(item.size)} · {formatTime(item.mtimeMs)}
                      </span>
                      {downloading && (
                        <div className="file-download-progress">
                          <div className="progress-track" aria-hidden="true">
                            <div style={{ width: `${percent}%` }} />
                          </div>
                          <span className="dim">{percent}%</span>
                        </div>
                      )}
                    </div>
                  </div>
                )}
                <span className="file-row-size">
                  {item.isDirectory && dirSizes[item.path] ? dirSizes[item.path] : ''}
                </span>
                <div className="li-actions file-actions">
                  {item.isDirectory && (
                    <>
                      <button
                        type="button"
                        className="ghost"
                        disabled={sizeBusyPath === item.path}
                        onClick={() => void statDir(item.path)}
                      >
                        {sizeBusyPath === item.path ? '…' : '统计'}
                      </button>
                      <button type="button" className="ghost" onClick={() => addTab(item.path)}>
                        新标签打开
                      </button>
                    </>
                  )}
                  {!item.isDirectory && (
                    <button
                      type="button"
                      className="file-download-btn"
                      disabled={Boolean(downloadBusyPath)}
                      onClick={() => void downloadFile(item)}
                    >
                      {downloading ? `${percent}%` : '下载'}
                    </button>
                  )}
                  <button
                    type="button"
                    className="ghost"
                    onClick={() => onOpenTerminal(item.isDirectory ? item.path : currentPath)}
                  >
                    新终端
                  </button>
                </div>
              </li>
            )
          })}
          {!entries.length && !busy && !error && <li className="empty">空目录</li>}
          {busy === 'boot' || busy === 'list' ? <li className="empty">加载中…</li> : null}
          {busy === 'paste' ? <li className="empty">粘贴中…</li> : null}
        </ul>
      </section>

      {ctxMenu && (
        <div
          className="term-ctx-menu file-ctx-menu"
          style={{ left: ctxMenu.x, top: ctxMenu.y }}
          onClick={(e) => e.stopPropagation()}
        >
          {ctxTarget?.isDirectory && (
            <button type="button" onClick={() => enterDir(ctxTarget)}>
              打开
            </button>
          )}
          {!ctxTarget?.isDirectory && ctxTarget && (
            <button type="button" onClick={() => void downloadFile(ctxTarget)}>
              下载到本机
            </button>
          )}
          <button
            type="button"
            disabled={!ctxSelected.length}
            onClick={() => copyItems(ctxSelected.map(toClipboardItem))}
          >
            复制
          </button>
          <button
            type="button"
            disabled={!ctxSelected.length}
            onClick={() => cutItems(ctxSelected.map(toClipboardItem))}
          >
            剪切
          </button>
          <button type="button" disabled={!canPaste} onClick={() => void pasteClipboard()}>
            粘贴
          </button>
          {ctxTarget && (
            <button type="button" onClick={() => void copyPath(ctxTarget.path)}>
              复制路径
            </button>
          )}
          {ctxTarget?.isDirectory && (
            <button type="button" onClick={() => addTab(ctxTarget.path)}>
              新标签打开
            </button>
          )}
          {ctxTarget && (
            <button type="button" onClick={() => onOpenTerminal(ctxTarget.isDirectory ? ctxTarget.path : currentPath)}>
              新终端
            </button>
          )}
        </div>
      )}

      <p className="dim file-hint">
        单击选中，Ctrl+点击多选，双击目录进入；支持复制/剪切/粘贴（Ctrl+C/X/V）与右键菜单；文件可下载到本机。
      </p>
    </div>
  )
}
