import type { Terminal } from '@xterm/xterm'
import { CanvasAddon } from '@xterm/addon-canvas'
import { WebglAddon } from '@xterm/addon-webgl'

/** WebView2 下必须用 WebGL/Canvas 渲染器，默认 DOM 路径易出现无 ANSI 色、选中不可见 */
export function attachXtermRenderer(term: Terminal): () => void {
  let webgl: WebglAddon | null = null
  let canvas: CanvasAddon | null = null
  let lossSub: { dispose: () => void } | null = null

  const mountCanvas = () => {
    if (canvas) return
    canvas = new CanvasAddon()
    term.loadAddon(canvas)
  }

  try {
    webgl = new WebglAddon()
    term.loadAddon(webgl)
    lossSub = webgl.onContextLoss(() => {
      webgl?.dispose()
      webgl = null
      mountCanvas()
    })
  } catch {
    mountCanvas()
  }

  return () => {
    lossSub?.dispose()
    webgl?.dispose()
    canvas?.dispose()
    webgl = null
    canvas = null
    lossSub = null
  }
}
