/// <reference types="vite/client" />

import type { api } from './api'

declare global {
  interface Window {
    api: typeof api
  }
}

export {}
