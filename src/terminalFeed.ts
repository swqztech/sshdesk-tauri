import { api } from './api'

export type TerminalChunk = string | Uint8Array

type Writer = (chunk: TerminalChunk) => void

const writers = new Map<string, Writer>()
const pending = new Map<string, TerminalChunk[]>()

const MAX_PENDING_CHUNKS = 800

let unsubGlobal: (() => void) | null = null

function decodePayload(payload: { data?: string; dataB64?: string }): TerminalChunk | null {
  if (payload.dataB64) {
    return Uint8Array.from(atob(payload.dataB64), (c) => c.charCodeAt(0))
  }
  if (payload.data) return payload.data
  return null
}

function route(sessionId: string, chunk: TerminalChunk) {
  const write = writers.get(sessionId)
  if (write) {
    write(chunk)
    return
  }
  const q = pending.get(sessionId) ?? []
  q.push(chunk)
  if (q.length > MAX_PENDING_CHUNKS) {
    q.splice(0, q.length - MAX_PENDING_CHUNKS)
  }
  pending.set(sessionId, q)
}

export function ensureTerminalFeed() {
  if (unsubGlobal) return
  unsubGlobal = api.onData((payload) => {
    if (!payload.sessionId) return
    const chunk = decodePayload(payload)
    if (chunk) route(payload.sessionId, chunk)
  })
}

export function registerTerminalWriter(sessionId: string, write: Writer): () => void {
  ensureTerminalFeed()
  writers.set(sessionId, write)
  const q = pending.get(sessionId)
  if (q?.length) {
    for (const chunk of q) write(chunk)
    pending.delete(sessionId)
  }
  return () => {
    writers.delete(sessionId)
  }
}
