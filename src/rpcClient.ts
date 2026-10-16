import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

type EventHandler = (payload: unknown) => void

const channelHandlers = new Map<string, Set<EventHandler>>()
let unlistenGlobal: UnlistenFn | null = null

async function ensureEventBridge(): Promise<void> {
  if (unlistenGlobal) return
  unlistenGlobal = await listen<{ channel: string; payload: unknown }>(
    'sshdesk-event',
    (ev) => {
      const { channel, payload } = ev.payload
      const set = channelHandlers.get(channel)
      if (!set) return
      for (const fn of set) fn(payload)
    }
  )
}

export async function rpc<T>(method: string, ...args: unknown[]): Promise<T> {
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
  void ensureEventBridge().then(() => {
    // 桥接就绪后无操作；仅保证尽早建立 Tauri 事件监听
  })
  let set = channelHandlers.get(channel)
  if (!set) {
    set = new Set()
    channelHandlers.set(channel, set)
  }
  set.add(handler)
  return () => {
    set?.delete(handler)
    if (set && set.size === 0) channelHandlers.delete(channel)
  }
}

export function unsubscribeEvent(channel: string, handler: EventHandler): void {
  channelHandlers.get(channel)?.delete(handler)
}
