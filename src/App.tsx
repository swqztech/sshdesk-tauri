import { FormEvent, useCallback, useEffect, useMemo, useState } from 'react'

import type { AuthType, HostInput, HostView } from './types'

import MongoPanel from './MongoPanel'

import QdrantPanel from './QdrantPanel'

import FileManagerPanel from './FileManagerPanel'
import HomePanel from './HomePanel'
import OverviewPanel from './OverviewPanel'
import TerminalPane from './TerminalPane'



type ShellTab = {

  shellId: string

  persistId: string

  label: string

  cwd?: string

  initialCommand?: string

}



type HostTab = {

  connectionId: string

  hostId: string

  title: string

  shells: ShellTab[]

  activeShellId: string

}



type FormState = {

  name: string

  host: string

  port: string

  username: string

  authType: AuthType

  password: string

  privateKeyPath: string

  passphrase: string

}



const emptyForm: FormState = {

  name: '',

  host: '',

  port: '22',

  username: 'root',

  authType: 'password',

  password: '',

  privateKeyPath: '',

  passphrase: ''

}



function shellLabelFromPath(path: string, index: number) {

  const base = path.split('/').filter(Boolean).pop()

  if (!base) return `终端 ${index}`

  return base.length > 22 ? `…${base.slice(-20)}` : base

}



function newPersistId() {

  return `t-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`

}

/** 恢复会话时最多打开几个终端，避免标签过多 */
const MAX_RESTORED_TERMINAL_TABS = 8



function toTerminalState(tab: HostTab) {

  const active = tab.shells.find((s) => s.shellId === tab.activeShellId)

  return {

    tabs: tab.shells.map((s) => ({

      id: s.persistId,

      label: s.label,

      cwd: s.cwd,

      initialCommand: s.initialCommand

    })),

    activeTabId: active?.persistId ?? tab.shells[0]?.persistId ?? ''

  }

}



export default function App() {

  const [hosts, setHosts] = useState<HostView[]>([])

  const [hostTabs, setHostTabs] = useState<HostTab[]>([])

  const [activeConnectionId, setActiveConnectionId] = useState<string | null>(null)

  const [query, setQuery] = useState('')

  const [busyId, setBusyId] = useState<string | null>(null)

  const [error, setError] = useState('')

  const [editor, setEditor] = useState<{ mode: 'create' | 'edit'; id?: string } | null>(null)

  const [form, setForm] = useState<FormState>(emptyForm)

  const [panel, setPanel] = useState<'overview' | 'home' | 'term' | 'files' | 'mongo' | 'qdrant'>(
    'overview'
  )

  const [pendingTermCd, setPendingTermCd] = useState<Record<string, string>>({})



  const refresh = useCallback(async () => {

    const list = await window.api.listHosts()

    setHosts(list)

  }, [])



  useEffect(() => {

    void refresh()

  }, [refresh])



  useEffect(() => {

    return window.api.onClosed(({ sessionId: shellId }) => {
      setHostTabs((prev) =>
        prev.map((tab) => {
          const shells = tab.shells.filter((s) => s.shellId !== shellId)
          const activeShellId =
            tab.activeShellId === shellId
              ? shells[shells.length - 1]?.shellId ?? ''
              : tab.activeShellId
          return { ...tab, shells, activeShellId }
        })
      )
    })

  }, [])



  useEffect(() => {

    for (const tab of hostTabs) {

      if (!tab.shells.length) continue

      void window.api.saveTerminalState(tab.hostId, toTerminalState(tab))

    }

  }, [hostTabs])



  useEffect(() => {

    return window.api.onConnectionClosed(({ connectionId }) => {

      setHostTabs((prev) => {

        const next = prev.filter((t) => t.connectionId !== connectionId)

        setActiveConnectionId((cur) => {

          if (cur !== connectionId) return cur

          return next[next.length - 1]?.connectionId ?? null

        })

        return next

      })

    })

  }, [])



  const filtered = useMemo(() => {

    const q = query.trim().toLowerCase()

    if (!q) return hosts

    return hosts.filter(

      (h) =>

        h.name.toLowerCase().includes(q) ||

        h.host.toLowerCase().includes(q) ||

        h.username.toLowerCase().includes(q)

    )

  }, [hosts, query])



  const activeHostTab = hostTabs.find((t) => t.connectionId === activeConnectionId)



  const openEditor = (mode: 'create' | 'edit', host?: HostView) => {

    setError('')

    if (mode === 'edit' && host) {

      setForm({

        name: host.name,

        host: host.host,

        port: String(host.port),

        username: host.username,

        authType: host.authType,

        password: '',

        privateKeyPath: host.privateKeyPath ?? '',

        passphrase: ''

      })

      setEditor({ mode, id: host.id })

    } else {

      setForm(emptyForm)

      setEditor({ mode: 'create' })

    }

  }



  const submitHost = async (e: FormEvent) => {

    e.preventDefault()

    setError('')

    const input: HostInput = {

      name: form.name,

      host: form.host,

      port: Number(form.port) || 22,

      username: form.username,

      authType: form.authType,

      password: form.password || undefined,

      privateKeyPath: form.privateKeyPath || undefined,

      passphrase: form.passphrase || undefined,

      keepPassword: editor?.mode === 'edit' && !form.password,

      keepPassphrase: editor?.mode === 'edit' && !form.passphrase

    }

    try {

      if (editor?.mode === 'edit' && editor.id) {

        await window.api.updateHost(editor.id, input)

      } else {

        await window.api.createHost(input)

      }

      setEditor(null)

      await refresh()

    } catch (err) {

      setError(err instanceof Error ? err.message : String(err))

    }

  }



  const removeHost = async (host: HostView) => {

    if (!window.confirm(`删除主机「${host.name}」？不会影响服务器本身。`)) return

    await window.api.deleteHost(host.id)

    await refresh()

  }



  const addShellTab = async (

    connectionId: string,

    opts?: { label?: string; remotePath?: string; initialCommand?: string }

  ) => {

    const { shellId } = await window.api.openShell(

      connectionId,

      opts?.initialCommand,

      opts?.initialCommand ? undefined : opts?.remotePath

    )

    setHostTabs((prev) =>

      prev.map((tab) => {

        if (tab.connectionId !== connectionId) return tab

        const label = opts?.label || `终端 ${tab.shells.length + 1}`

        const persistId = newPersistId()

        return {

          ...tab,

          shells: [

            ...tab.shells,

            {

              shellId,

              persistId,

              label,

              cwd: opts?.remotePath,

              initialCommand: opts?.initialCommand

            }

          ],

          activeShellId: shellId

        }

      })

    )

    setActiveConnectionId(connectionId)

    setPanel('term')

    return shellId

  }



  const openPm2Logs = async (connectionId: string, processName: string) => {

    setError('')

    try {

      const safe = JSON.stringify(`pm2 logs ${processName}`)

      await addShellTab(connectionId, {

        label: `logs ${processName}`,

        initialCommand: `bash -lc ${safe}`

      })

    } catch (err) {

      setError(err instanceof Error ? err.message : String(err))

    }

  }



  const connectHost = async (host: HostView) => {

    setBusyId(host.id)

    setError('')

    try {

      const existing = hostTabs.find((t) => t.hostId === host.id)

      if (existing) {

        setActiveConnectionId(existing.connectionId)

        setPanel('home')

        return

      }

      const [savedTerm, fbStatePre] = await Promise.all([

        window.api.loadTerminalState(host.id),

        window.api.loadFileBrowserState(host.id)

      ])

      const defaultCwd =

        fbStatePre.tabs.find((t) => t.id === fbStatePre.activeTabId)?.path ??

        fbStatePre.tabs[0]?.path

      const savedTabsRaw =

        savedTerm.tabs.length > 0

          ? savedTerm.tabs

          : [{ id: newPersistId(), label: '终端 1' }]

      const savedTabs = savedTabsRaw.slice(0, MAX_RESTORED_TERMINAL_TABS)

      const first = savedTabs[0]!

      const firstCwd = first.cwd ?? defaultCwd

      const { connectionId, shellId, title } = await window.api.connect(host.id, {

        initialCommand: first.initialCommand,

        initialCwd: first.initialCommand ? undefined : firstCwd

      })

      const shells: ShellTab[] = []

      shells.push({

        shellId,

        persistId: first.id,

        label: first.label,

        cwd: firstCwd,

        initialCommand: first.initialCommand

      })

      for (const tab of savedTabs.slice(1)) {

        const cwd = tab.cwd ?? defaultCwd

        const { shellId: sid } = await window.api.openShell(

          connectionId,

          tab.initialCommand,

          tab.initialCommand ? undefined : cwd

        )

        shells.push({

          shellId: sid,

          persistId: tab.id,

          label: tab.label,

          cwd,

          initialCommand: tab.initialCommand

        })

      }

      const activePersistId = savedTabs.some((t) => t.id === savedTerm.activeTabId)

        ? savedTerm.activeTabId

        : savedTabs[0]!.id

      const activeIdx = savedTabs.findIndex((t) => t.id === activePersistId)

      const activeShellId = shells[activeIdx >= 0 ? activeIdx : 0]!.shellId

      setHostTabs((prev) => [

        ...prev,

        {

          connectionId,

          hostId: host.id,

          title,

          shells,

          activeShellId

        }

      ])

      setActiveConnectionId(connectionId)

      setPanel(hostTabs.length >= 1 ? 'overview' : 'home')

    } catch (err) {

      setError(err instanceof Error ? err.message : String(err))

    } finally {

      setBusyId(null)

    }

  }



  const openNewTerminalAtPath = async (connectionId: string, remotePath: string) => {

    setError('')

    try {

      const tab = hostTabs.find((t) => t.connectionId === connectionId)

      const index = (tab?.shells.length ?? 0) + 1

      await addShellTab(connectionId, {

        label: shellLabelFromPath(remotePath, index),

        remotePath

      })

    } catch (err) {

      setError(err instanceof Error ? err.message : String(err))

    }

  }



  const clearPendingTermCd = (shellId: string) => {

    setPendingTermCd((prev) => {

      if (!prev[shellId]) return prev

      const next = { ...prev }

      delete next[shellId]

      return next

    })

  }



  const closeOtherShellTabs = async (connectionId: string, keepShellId: string) => {

    const tab = hostTabs.find((t) => t.connectionId === connectionId)

    if (!tab) return

    const toClose = tab.shells.filter((s) => s.shellId !== keepShellId)

    for (const shell of toClose) {

      await window.api.closeShell(shell.shellId)

    }

    setHostTabs((prev) =>

      prev.map((t) => {

        if (t.connectionId !== connectionId) return t

        const shells = t.shells.filter((s) => s.shellId === keepShellId)

        return { ...t, shells, activeShellId: keepShellId }

      })

    )

  }



  const closeShellTab = async (connectionId: string, shellId: string) => {

    await window.api.closeShell(shellId)

    setHostTabs((prev) =>

      prev

        .map((tab) => {

          if (tab.connectionId !== connectionId) return tab

          const shells = tab.shells.filter((s) => s.shellId !== shellId)

          if (!shells.length) return tab

          const activeShellId =

            tab.activeShellId === shellId ? shells[shells.length - 1]!.shellId : tab.activeShellId

          return { ...tab, shells, activeShellId }
        })
    )
  }



  const closeHostTab = async (connectionId: string) => {

    await window.api.closeConnection(connectionId)

    setHostTabs((prev) => {

      const next = prev.filter((t) => t.connectionId !== connectionId)

      setActiveConnectionId((cur) => {

        if (cur !== connectionId) return cur

        return next[next.length - 1]?.connectionId ?? null

      })

      return next

    })

  }



  return (

    <div className="app">

      <aside className="sidebar">

        <header className="brand">

          <p className="brand-name">连枢</p>

          <p className="brand-sub">SSH 主机台</p>

        </header>



        <div className="side-actions">

          <button type="button" className="primary" onClick={() => openEditor('create')}>

            添加主机

          </button>

          <input

            className="search"

            placeholder="搜索名称 / IP"

            value={query}

            onChange={(e) => setQuery(e.target.value)}

          />

        </div>



        <ul className="host-list">

          {filtered.map((h) => {

            const isConnected = hostTabs.some((t) => t.hostId === h.id)

            const isActiveHost = activeHostTab?.hostId === h.id

            return (

            <li
              key={h.id}
              className={`host-card${isActiveHost ? ' active' : ''}${isConnected && !isActiveHost ? ' connected' : ''}`}
            >

              <div className="host-main">

                <strong>{h.name}</strong>

                <span className="mono">

                  {h.username}@{h.host}:{h.port}

                </span>

                <span className="tag">{h.authType === 'password' ? '密码' : '私钥'}</span>

              </div>

              <div className="host-ops">

                <button

                  type="button"

                  className="accent"

                  disabled={busyId === h.id}

                  onClick={() => void connectHost(h)}

                >

                  {busyId === h.id ? '连接中…' : '连接'}

                </button>

                <button type="button" className="ghost" onClick={() => openEditor('edit', h)}>

                  编辑

                </button>

                <button type="button" className="ghost danger" onClick={() => void removeHost(h)}>

                  删

                </button>

              </div>

            </li>

            )

          })}

          {filtered.length === 0 && <li className="empty">还没有主机，先添加一台 Linux 服务器。</li>}

        </ul>

      </aside>



      <main className="workspace">

        {error && <div className="banner error">{error}</div>}



        {hostTabs.length > 0 && activeHostTab ? (

          <>

            <div className="tabs">

              {hostTabs.map((t) => {

                const host = hosts.find((h) => h.id === t.hostId)

                const label = host?.name || t.title

                const tip = host

                  ? `${host.username}@${host.host}:${host.port}`

                  : t.title

                return (

                  <div

                    key={t.connectionId}

                    className={`tab ${activeConnectionId === t.connectionId ? 'on' : ''}`}

                  >

                    <button

                      type="button"

                      className="tab-btn"

                      title={tip}

                      onClick={() => setActiveConnectionId(t.connectionId)}

                    >

                      {label}

                    </button>

                    <button

                      type="button"

                      className="tab-x"

                      aria-label="关闭主机"

                      onClick={() => void closeHostTab(t.connectionId)}

                    >

                      ×

                    </button>

                  </div>

                )

              })}

            </div>

            <div className="panel-switch">

              <button
                type="button"
                className={panel === 'overview' ? 'on' : ''}
                onClick={() => setPanel('overview')}
              >
                总览
              </button>

              <button

                type="button"

                className={panel === 'home' ? 'on' : ''}

                onClick={() => setPanel('home')}

              >

                首页

              </button>

              <button

                type="button"

                className={panel === 'term' ? 'on' : ''}

                onClick={() => setPanel('term')}

              >

                终端

              </button>

              <button

                type="button"

                className={panel === 'files' ? 'on' : ''}

                onClick={() => setPanel('files')}

              >

                文件管理

              </button>

              <button

                type="button"

                className={panel === 'mongo' ? 'on' : ''}

                onClick={() => setPanel('mongo')}

              >

                MongoDB

              </button>

              <button

                type="button"

                className={panel === 'qdrant' ? 'on' : ''}

                onClick={() => setPanel('qdrant')}

              >

                Qdrant

              </button>

            </div>



            {panel === 'term' && (

              <div className="term-tab-bar file-tab-bar">

                {activeHostTab.shells.map((shell) => (

                  <div

                    key={shell.shellId}

                    className={`file-tab ${shell.shellId === activeHostTab.activeShellId ? 'on' : ''}`}

                  >

                    <button

                      type="button"

                      className="file-tab-btn"

                      title={shell.label}

                      onClick={() =>

                        setHostTabs((prev) =>

                          prev.map((tab) =>

                            tab.connectionId === activeHostTab.connectionId

                              ? { ...tab, activeShellId: shell.shellId }

                              : tab

                          )

                        )

                      }

                    >

                      {shell.label}

                    </button>

                    {activeHostTab.shells.length > 1 && (

                      <button

                        type="button"

                        className="file-tab-x"

                        aria-label="关闭终端"

                        onClick={() => void closeShellTab(activeHostTab.connectionId, shell.shellId)}

                      >

                        ×

                      </button>

                    )}

                  </div>

                ))}

                <button

                  type="button"

                  className="ghost term-tab-add"

                  onClick={() => void addShellTab(activeHostTab.connectionId)}

                >

                  + 新终端

                </button>

                {activeHostTab.shells.length > 1 && (

                  <button

                    type="button"

                    className="ghost term-tab-add"

                    title="关闭本主机下除当前以外的终端标签"

                    onClick={() =>

                      void closeOtherShellTabs(

                        activeHostTab.connectionId,

                        activeHostTab.activeShellId

                      )

                    }

                  >

                    仅保留当前

                  </button>

                )}

              </div>

            )}



            <div className={`term-stack-wrap ${panel !== 'term' ? 'hidden-term-wrap' : ''}`}>

              {hostTabs.map((hostTab) => {

                const isActiveHost = hostTab.connectionId === activeConnectionId

                return (

                  <div

                    key={hostTab.connectionId}

                    className={`term-stack ${!isActiveHost ? 'hidden-term' : ''}`}

                  >

                    {hostTab.shells.map((shell) => (

                      <TerminalPane

                        key={shell.shellId}

                        sessionId={shell.shellId}

                        active={

                          panel === 'term' &&

                          isActiveHost &&

                          hostTab.activeShellId === shell.shellId

                        }

                        hostVisible={panel === 'term' && isActiveHost}

                        pendingCd={pendingTermCd[shell.shellId]}

                        onPendingCdConsumed={() => clearPendingTermCd(shell.shellId)}

                      />

                    ))}

                  </div>

                )

              })}

            </div>



            {panel === 'overview' && (
              <OverviewPanel
                connections={hostTabs}
                hosts={hosts}
                activeConnectionId={activeConnectionId}
                active
                onSelect={(connectionId) => {
                  setActiveConnectionId(connectionId)
                  setPanel('home')
                }}
                onOpenPm2Logs={(connectionId, processName) =>
                  void openPm2Logs(connectionId, processName)
                }
              />
            )}

            {panel === 'home' && (
              <HomePanel
                connectionId={activeHostTab.connectionId}
                hostTitle={
                  hosts.find((h) => h.id === activeHostTab.hostId)?.name || activeHostTab.title
                }
                active
                onOpenPm2Logs={(processName) =>
                  void openPm2Logs(activeHostTab.connectionId, processName)
                }
              />
            )}

            {panel === 'files' && (

              <FileManagerPanel

                sessionId={activeHostTab.connectionId}

                hostId={activeHostTab.hostId}

                active

                onOpenTerminal={(path) =>

                  void openNewTerminalAtPath(activeHostTab.connectionId, path)

                }

              />

            )}

            {panel === 'mongo' && (

              <MongoPanel

                sessionId={activeHostTab.connectionId}

                hostTitle={

                  hosts.find((h) => h.id === activeHostTab.hostId)?.name || activeHostTab.title

                }

                active

              />

            )}

            {panel === 'qdrant' && (

              <QdrantPanel sessionId={activeHostTab.connectionId} active />

            )}

          </>

        ) : (

          <div className="hero">

            <h1>把服务器收进左侧抽屉</h1>

            <p>添加主机 → 连接 → 终端 / 文件 / MongoDB / Qdrant 备份上传 OSS。</p>

            <button type="button" className="primary lg" onClick={() => openEditor('create')}>

              添加第一台主机

            </button>

          </div>

        )}

      </main>



      {editor && (

        <div className="modal-backdrop" role="presentation">

          <form

            className="modal"

            onClick={(e) => e.stopPropagation()}

            onSubmit={(e) => void submitHost(e)}

          >

            <div className="modal-head">

              <h2>{editor.mode === 'create' ? '添加主机' : '编辑主机'}</h2>

              <button type="button" className="ghost" onClick={() => setEditor(null)}>

                取消

              </button>

            </div>

            <div className="modal-body">

              <label>

                显示名称

                <input

                  required

                  value={form.name}

                  onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}

                  placeholder="生产机 / 测试机"

                />

              </label>

              <div className="row">

                <label className="grow">

                  主机

                  <input

                    required

                    value={form.host}

                    onChange={(e) => setForm((f) => ({ ...f, host: e.target.value }))}

                    placeholder="192.168.1.10 或 example.com"

                  />

                </label>

                <label className="port">

                  端口

                  <input

                    value={form.port}

                    onChange={(e) => setForm((f) => ({ ...f, port: e.target.value }))}

                  />

                </label>

              </div>

              <label>

                用户名

                <input

                  required

                  value={form.username}

                  onChange={(e) => setForm((f) => ({ ...f, username: e.target.value }))}

                />

              </label>

              <div className="auth-switch">

                <button

                  type="button"

                  className={form.authType === 'password' ? 'on' : ''}

                  onClick={() => setForm((f) => ({ ...f, authType: 'password' }))}

                >

                  密码登录

                </button>

                <button

                  type="button"

                  className={form.authType === 'privateKey' ? 'on' : ''}

                  onClick={() => setForm((f) => ({ ...f, authType: 'privateKey' }))}

                >

                  私钥登录

                </button>

              </div>

              {form.authType === 'password' ? (

                <label>

                  密码

                  {editor.mode === 'edit' ? '（留空则保留原密码）' : ''}

                  <input

                    type="password"

                    autoComplete="new-password"

                    value={form.password}

                    onChange={(e) => setForm((f) => ({ ...f, password: e.target.value }))}

                    required={editor.mode === 'create'}

                  />

                </label>

              ) : (

                <>

                  <label>

                    私钥路径

                    <div className="row">

                      <input

                        className="grow"

                        required

                        value={form.privateKeyPath}

                        onChange={(e) =>

                          setForm((f) => ({ ...f, privateKeyPath: e.target.value }))

                        }

                        placeholder="C:\\Users\\...\\.ssh\\id_rsa"

                      />

                      <button

                        type="button"

                        className="ghost"

                        onClick={() => {

                          void window.api.pickKey().then((p) => {

                            if (p) setForm((f) => ({ ...f, privateKeyPath: p }))

                          })

                        }}

                      >

                        浏览

                      </button>

                    </div>

                  </label>

                  <label>

                    私钥口令

                    {editor.mode === 'edit' ? '（留空则保留）' : '（可选）'}

                    <input

                      type="password"

                      value={form.passphrase}

                      onChange={(e) => setForm((f) => ({ ...f, passphrase: e.target.value }))}

                    />

                  </label>

                </>

              )}

              {error && <p className="form-error">{error}</p>}

            </div>

            <div className="modal-foot">

              <button type="submit" className="primary">

                保存

              </button>

            </div>

          </form>

        </div>

      )}

    </div>

  )

}


