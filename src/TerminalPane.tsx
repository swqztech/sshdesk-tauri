import { useEffect, useRef } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebLinksAddon } from '@xterm/addon-web-links'
import '@xterm/xterm/css/xterm.css'
import { registerTerminalWriter } from './terminalFeed'
import { attachXtermRenderer } from './terminalRenderer'

type Props = {
  sessionId: string
  /** 当前标签是否在前台显示 */
  active: boolean
  /** 所属主机是否在终端视图下可见（切回主机时需重算尺寸） */
  hostVisible?: boolean
  pendingCd?: string
  onPendingCdConsumed?: () => void
}

/** 统一换行符，避免 Windows 剪贴板 CRLF 造成重复执行 */
function normalizePasteText(text: string): string {
  return text.replace(/\r\n/g, '\n').replace(/\r/g, '\n')
}

async function copySelection(term: Terminal) {
  const text = term.getSelection()
  if (!text) return false
  await navigator.clipboard.writeText(text)
  return true
}

async function pasteFromClipboard(term: Terminal) {
  let text = ''
  try {
    text = await navigator.clipboard.readText()
  } catch {
    return false
  }
  if (!text) return false
  term.paste(normalizePasteText(text))
  return true
}

function bindClipboard(term: Terminal, host: HTMLDivElement) {
  // 捕获阶段统一处理粘贴，避免 Ctrl+V / Shift+Insert 与 xterm 内置 paste 重复触发
  const onPaste = (ev: ClipboardEvent) => {
    ev.preventDefault()
    ev.stopImmediatePropagation()
    const raw = ev.clipboardData?.getData('text/plain') ?? ''
    if (!raw) return
    term.paste(normalizePasteText(raw))
  }
  host.addEventListener('paste', onPaste, true)

  term.attachCustomKeyEventHandler((ev) => {
    if (ev.type !== 'keydown') return true

    const mod = ev.ctrlKey || ev.metaKey
    const key = ev.key

    // 有选中时 Ctrl+C 复制，无选中则交给终端发 SIGINT
    if (mod && !ev.shiftKey && !ev.altKey && (key === 'c' || key === 'C')) {
      if (term.hasSelection()) {
        void copySelection(term)
        return false
      }
      return true
    }

    if (mod && ev.shiftKey && (key === 'c' || key === 'C')) {
      void copySelection(term)
      return false
    }

    if (ev.ctrlKey && key === 'Insert') {
      void copySelection(term)
      return false
    }

    // 粘贴交给上方 paste 事件统一处理，勿在此再 term.paste
    if (mod && !ev.altKey && (key === 'v' || key === 'V')) {
      return true
    }

    if (ev.shiftKey && key === 'Insert') {
      return true
    }

    return true
  })

  const onContextMenu = (e: MouseEvent) => {
    e.preventDefault()
    closeTermContextMenu()

    const menu = document.createElement('div')
    menu.className = 'term-ctx-menu'
    menu.style.left = `${e.clientX}px`
    menu.style.top = `${e.clientY}px`

    const addItem = (label: string, action: () => void | Promise<void>, disabled = false) => {
      const btn = document.createElement('button')
      btn.type = 'button'
      btn.textContent = label
      btn.disabled = disabled
      btn.onclick = () => {
        void action()
        closeTermContextMenu()
      }
      menu.appendChild(btn)
    }

    addItem(
      '复制',
      () => {
        void copySelection(term)
      },
      !term.hasSelection()
    )
    addItem('粘贴', () => {
      void pasteFromClipboard(term)
    })
    addItem('全选', () => {
      term.selectAll()
    })

    document.body.appendChild(menu)
    termCtxMenu = menu

    const rect = menu.getBoundingClientRect()
    if (rect.right > window.innerWidth) {
      menu.style.left = `${Math.max(8, window.innerWidth - rect.width - 8)}px`
    }
    if (rect.bottom > window.innerHeight) {
      menu.style.top = `${Math.max(8, window.innerHeight - rect.height - 8)}px`
    }
  }

  host.addEventListener('contextmenu', onContextMenu)

  return () => {
    host.removeEventListener('paste', onPaste, true)
    host.removeEventListener('contextmenu', onContextMenu)
    closeTermContextMenu()
  }
}

let termCtxMenu: HTMLDivElement | null = null

function closeTermContextMenu() {
  termCtxMenu?.remove()
  termCtxMenu = null
}

const resizeSent = new Map<string, { cols: number; rows: number }>()

function fitTerminal(
  fit: FitAddon | null,
  term: Terminal | null,
  sessionId: string,
  allowRemoteResize: boolean
) {
  if (!fit || !term) return
  try {
    fit.fit()
    if (!allowRemoteResize) return
    const cols = term.cols
    const rows = term.rows
    const prev = resizeSent.get(sessionId)
    if (prev && prev.cols === cols && prev.rows === rows) return
    resizeSent.set(sessionId, { cols, rows })
    void window.api.resize(sessionId, cols, rows)
  } catch {
    // ignore
  }
}


export default function TerminalPane({
  sessionId,
  active,
  hostVisible = true,
  pendingCd,
  onPendingCdConsumed
}: Props) {
  const hostRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const activeRef = useRef(active)
  activeRef.current = active && hostVisible

  useEffect(() => {
    if (!hostRef.current) return

    const term = new Terminal({
      cursorBlink: true,
      fontFamily: '"IBM Plex Mono", ui-monospace, monospace',
      fontSize: 14,
      lineHeight: 1.25,
      convertEol: true,
      scrollback: 5000,
      minimumContrastRatio: 1,
      drawBoldTextInBrightColors: true,
      theme: {
        background: '#0c1218',
        foreground: '#d7e2ea',
        cursor: '#f2c14e',
        selectionBackground: '#f2c14e',
        selectionInactiveBackground: '#6b5a2e',
        selectionForeground: '#0a0f14',
        black: '#0c1218',
        red: '#ff6b4a',
        green: '#3ddc97',
        yellow: '#f2c14e',
        blue: '#6cb6ff',
        magenta: '#c792ea',
        cyan: '#7fdbda',
        white: '#d7e2ea',
        brightBlack: '#5a6b7a',
        brightRed: '#ff8f75',
        brightGreen: '#6aefb0',
        brightYellow: '#ffd56a',
        brightBlue: '#8fc8ff',
        brightMagenta: '#d4a8f0',
        brightCyan: '#a8eceb',
        brightWhite: '#ffffff'
      },
      allowProposedApi: true
    })
    const fit = new FitAddon()
    term.loadAddon(fit)
    term.loadAddon(new WebLinksAddon())
    term.open(hostRef.current)
    const disposeRenderer = attachXtermRenderer(term)
    fit.fit()
    fitTerminal(fit, term, sessionId, true)

    const onData = term.onData((data) => {
      void window.api.write(sessionId, data)
    })

    const offFeed = registerTerminalWriter(sessionId, (chunk) => {
      term.write(chunk)
    })

    const offClosed = window.api.onClosed((payload) => {
      if (payload.sessionId !== sessionId) return
      term.writeln('\r\n\x1b[33m[会话已关闭]\x1b[0m')
    })

    const offError = window.api.onError((payload) => {
      if (payload.sessionId !== sessionId) return
      term.writeln(`\r\n\x1b[31m[错误] ${payload.message}\x1b[0m`)
    })

    termRef.current = term
    fitRef.current = fit

    const unbindClipboard = bindClipboard(term, hostRef.current)

    let fitTimer: ReturnType<typeof setTimeout> | undefined
    const scheduleFit = () => {
      clearTimeout(fitTimer)
      fitTimer = setTimeout(() => {
        if (!activeRef.current) return
        fitTerminal(fitRef.current, termRef.current, sessionId, true)
      }, 120)
    }

    const ro = new ResizeObserver(() => {
      scheduleFit()
    })
    ro.observe(hostRef.current)

    const onWindowResize = () => {
      scheduleFit()
    }
    window.addEventListener('resize', onWindowResize)

    return () => {
      onData.dispose()
      offFeed()
      offClosed()
      offError()
      unbindClipboard()
      ro.disconnect()
      window.removeEventListener('resize', onWindowResize)
      clearTimeout(fitTimer)
      resizeSent.delete(sessionId)
      disposeRenderer()
      term.dispose()
      termRef.current = null
      fitRef.current = null
    }
  }, [sessionId])

  useEffect(() => {
    const onDocClick = () => closeTermContextMenu()
    document.addEventListener('click', onDocClick)
    return () => document.removeEventListener('click', onDocClick)
  }, [])

  useEffect(() => {
    if (!active || !pendingCd?.trim()) return
    void window.api.cdShell(sessionId, pendingCd.trim()).then((ok) => {
      if (ok) onPendingCdConsumed?.()
    })
  }, [active, pendingCd, sessionId, onPendingCdConsumed])

  useEffect(() => {
    if (!active || !hostVisible) return
    const refit = () => {
      const term = termRef.current
      fitTerminal(fitRef.current, term, sessionId, true)
      try {
        term?.refresh(0, Math.max(0, term.rows - 1))
      } catch {
        // ignore
      }
    }
    refit()
    const t1 = window.setTimeout(refit, 50)
    const t2 = window.setTimeout(refit, 180)
    termRef.current?.focus()
    return () => {
      window.clearTimeout(t1)
      window.clearTimeout(t2)
    }
  }, [active, hostVisible, sessionId])

  return (
    <div
      className={`term-host ${active ? 'is-active' : ''}`}
      ref={hostRef}
      onMouseDown={() => {
        if (active) termRef.current?.focus()
      }}
    />
  )
}

