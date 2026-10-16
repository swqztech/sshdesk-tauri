import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

type EventHandler = (payload: unknown) => void

const channelHandlers = new Map<string, Set<EventHandler>>()
let unlistenGlobal: UnlistenFn | null = null
let bridgeReady: Promise<void> | null = null

function ensureEventBridge(): Promise<void> {
  if (bridgeReady) return bridgeReady
  bridgeReady = listen<{ channel: string; payload: unknown }>('sshdesk-event', (ev) => {
    const { channel, payload } = ev.payload
    const set = channelHandlers.get(channel)
    if (!set) return
    for (const fn of set) fn(payload)
  }).then((unlisten) => {
    unlistenGlobal = unlisten
  })
  return bridgeReady
}

export async function rpc<T>(method: string, ...args: unknown[]): Promise<T> {
  await ensureEventBridge()
  const json = await invoke<string>('sshdesk_rpc', {
    request: { method, args }
  })
  const parsed = JSON.parse(json) as { ok: boolean; result?: T; error?: string }
  if (!parsed.ok) {
    throw new Error(parsed.error || `RPC 失败: ${method}`)
  }
  return parsed.result as T
}

export function subscribeEvent(channel: string, handler: EventHandler): () => void {
  let set = channelHandlers.get(channel)
  if (!set) {
    set = new Set()
    channelHandlers.set(channel, set)
  }
  set.add(handler)
  void ensureEventBridge()
  return () => {
    set?.delete(handler)
    if (set && set.size === 0) channelHandlers.delete(channel)
  }
}

export function unsubscribeEvent(channel: string, handler: EventHandler): void {
  channelHandlers.get(channel)?.delete(handler)
}
