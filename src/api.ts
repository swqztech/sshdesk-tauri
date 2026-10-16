import { open, save } from '@tauri-apps/plugin-dialog'
import { revealItemInDir } from '@tauri-apps/plugin-opener'
import { rpc, subscribeEvent, unsubscribeEvent } from './rpcClient'
import type {
  AuthType,
  BackupConfig,
  BackupProgress,
  FileBrowserState,
  FileDownloadProgress,
  HostInput,
  HostPm2Snapshot,
  HostStats,
  HostView,
  LocalBackupItem,
  MongoConn,
  MongoDbInfo,
  MongoBackupMeta,
  MongoLocalConn,
  OssBackupItem,
  QdrantBackupMetaPeek,
  QdrantBackupProgress,
  QdrantCollectionInfo,
  QdrantConfig,
  QdrantLocalSettings,
  RemoteFileEntry,
  SessionInfo,
  TerminalState
} from './types'

export const api = {
  listHosts: (): Promise<HostView[]> => rpc('hosts:list'),
  createHost: (input: HostInput): Promise<HostView> =>
    rpc('hosts:create', input),
  updateHost: (id: string, input: HostInput): Promise<HostView> =>
    rpc('hosts:update', id, input),
  deleteHost: (id: string): Promise<boolean> => rpc('hosts:delete', id),
  pickKey: async (): Promise<string | null> => {
    const picked = await open({ multiple: false, title: '选择 SSH 私钥' })
    return typeof picked === 'string' ? picked : null
  },

  listSessions: (): Promise<SessionInfo[]> => rpc('ssh:list'),
  connect: (
    hostId: string,
    opts?: { initialCommand?: string; initialCwd?: string }
  ): Promise<{ connectionId: string; shellId: string; title: string }> =>
    rpc('ssh:connect', hostId, opts),
  openShell: (
    connectionId: string,
    initialCommand?: string,
    initialCwd?: string
  ): Promise<{ shellId: string }> =>
    rpc('ssh:openShell', connectionId, initialCommand, initialCwd),
  write: (sessionId: string, data: string): Promise<boolean> =>
    rpc('ssh:write', sessionId, data),
  resize: (sessionId: string, cols: number, rows: number): Promise<boolean> =>
    rpc('ssh:resize', sessionId, cols, rows),
  closeShell: (shellId: string): Promise<boolean> => rpc('ssh:closeShell', shellId),
  closeConnection: (connectionId: string): Promise<boolean> =>
    rpc('ssh:closeConnection', connectionId),
  cdShell: (sessionId: string, remotePath: string): Promise<boolean> =>
    rpc('ssh:cdShell', sessionId, remotePath),
  runShell: (sessionId: string, command: string): Promise<boolean> =>
    rpc('ssh:runShell', sessionId, command),

  loadFileBrowserState: (
    hostId: string,
    sessionId?: string
  ): Promise<FileBrowserState> => rpc('files:loadState', hostId, sessionId),
  saveFileBrowserState: (hostId: string, state: FileBrowserState): Promise<boolean> =>
    rpc('files:saveState', hostId, state),
  loadTerminalState: (hostId: string): Promise<TerminalState> =>
    rpc('terminal:loadState', hostId),
  saveTerminalState: (hostId: string, state: TerminalState): Promise<boolean> =>
    rpc('terminal:saveState', hostId, state),
  listRemoteDir: (
    sessionId: string,
    dirPath: string
  ): Promise<{
    path: string
    parent: string | null
    entries: RemoteFileEntry[]
  }> => rpc('files:listDir', sessionId, dirPath),
  statRemoteDirSize: (
    sessionId: string,
    dirPath: string
  ): Promise<{ path: string; bytes: number; human: string }> =>
    rpc('files:dirSize', sessionId, dirPath),
  downloadRemoteFile: (
    sessionId: string,
    remotePath: string,
    localPath?: string
  ): Promise<{ ok: true; localPath: string; bytes: number }> =>
    rpc('files:download', sessionId, remotePath, localPath),
  copyRemotePaths: (
    sessionId: string,
    sources: string[],
    destDir: string
  ): Promise<{ count: number }> => rpc('files:copy', sessionId, sources, destDir),
  moveRemotePaths: (
    sessionId: string,
    sources: string[],
    destDir: string
  ): Promise<{ count: number }> => rpc('files:move', sessionId, sources, destDir),
  fetchHostStats: (connectionId: string): Promise<HostStats> =>
    rpc('host:stats', connectionId),
  fetchHostPm2: (connectionId: string): Promise<HostPm2Snapshot> =>
    rpc('host:pm2', connectionId),

  listMongoDatabases: (
    sessionId: string,
    conn?: MongoConn,
    cfg?: BackupConfig
  ): Promise<MongoDbInfo[]> => rpc('mongo:listDatabases', sessionId, conn, cfg),
  loadBackupConfig: (sessionId: string): Promise<BackupConfig> =>
    rpc('mongo:loadBackupConfig', sessionId),
  saveBackupConfig: (sessionId: string, cfg: BackupConfig): Promise<BackupConfig> =>
    rpc('mongo:saveBackupConfig', sessionId, cfg),
  defaultBackupConfig: (): Promise<BackupConfig> =>
    rpc('mongo:defaultBackupConfig'),
  defaultDownloadDir: (): Promise<string> => rpc('mongo:defaultDownloadDir'),
  listPcMongoBackups: (localDir?: string): Promise<LocalBackupItem[]> =>
    rpc('mongo:listPcBackups', localDir),
  listLocalBackups: (sessionId: string, cfg: BackupConfig): Promise<LocalBackupItem[]> =>
    rpc('mongo:listLocalBackups', sessionId, cfg),
  listOssBackups: (cfg: BackupConfig): Promise<OssBackupItem[]> =>
    rpc('mongo:listOssBackups', cfg),
  deleteLocalBackup: (sessionId: string, remotePath: string): Promise<boolean> =>
    rpc('mongo:deleteLocalBackup', sessionId, remotePath),
  deleteOssBackup: (cfg: BackupConfig, objectKey: string): Promise<boolean> =>
    rpc('mongo:deleteOssBackup', cfg, objectKey),
  backupMongo: (
    sessionId: string,
    dbName: string,
    conn: MongoConn | undefined,
    cfg: BackupConfig
  ): Promise<{
    fileName: string
    remotePath: string
    ossKey: string
    bytes: number
    uploaded: boolean
  }> => rpc('mongo:backup', sessionId, dbName, conn, cfg),
  backupMongoIncremental: (
    sessionId: string,
    dbName: string,
    conn: MongoConn | undefined,
    cfg: BackupConfig
  ): Promise<{
    fileName: string
    remotePath: string
    ossKey: string
    bytes: number
    uploaded: boolean
    changedCollections: string[]
  }> => rpc('mongo:backupIncremental', sessionId, dbName, conn, cfg),
  loadMongoBackupChain: (
    sessionId: string,
    dbName: string,
    cfg: BackupConfig
  ): Promise<{ version: 1; db: string; entries: unknown[] } | null> =>
    rpc('mongo:loadBackupChain', sessionId, dbName, cfg),
  readMongoBackupMeta: (localPath: string): Promise<MongoBackupMeta | null> =>
    rpc('mongo:readBackupMeta', localPath),
  downloadOssBackup: (
    sessionId: string,
    cfg: BackupConfig,
    objectKey: string,
    localDir?: string
  ): Promise<{ fileName: string; localPath: string; bytes: number }> =>
    rpc('mongo:downloadOss', sessionId, cfg, objectKey, localDir),
  defaultLocalMongoConn: (): Promise<MongoLocalConn> =>
    rpc('mongo:defaultLocalConn'),
  restoreMongoFromLocal: (
    sessionId: string,
    localPath: string,
    targetDb: string,
    mode: 'merge' | 'replace',
    localConn: MongoLocalConn | undefined,
    sourceDbHint?: string | null,
    siblings?: LocalBackupItem[]
  ): Promise<{
    ok: true
    db: string
    mode: 'merge' | 'replace'
    format: string
    connectionLabel: string
    steps?: number
  }> =>
    rpc(
      'mongo:restoreFromLocal',
      sessionId,
      localPath,
      targetDb,
      mode,
      localConn,
      sourceDbHint,
      siblings
    ),
  uploadServerBackupToOss: (
    sessionId: string,
    remotePath: string,
    cfg: BackupConfig
  ): Promise<{
    fileName: string
    remotePath: string
    ossKey: string
    bytes: number
    uploaded: boolean
    via: 'server-internal' | 'server-public' | 'local-relay'
  }> => rpc('mongo:uploadServerToOss', sessionId, remotePath, cfg),
  pickDownloadDir: async (): Promise<string | null> => {
    const picked = await open({ directory: true, multiple: false, title: '选择备份保存目录' })
    return typeof picked === 'string' ? picked : null
  },
  showItem: async (filePath: string): Promise<boolean> => {
    await revealItemInDir(filePath)
    return true
  },
  openPath: async (filePath: string): Promise<boolean> => {
    const { openPath: openFsPath } = await import('@tauri-apps/plugin-opener')
    await openFsPath(filePath)
    return true
  },

  onData: (
    cb: (payload: { sessionId: string; data?: string; dataB64?: string }) => void
  ) => {
    const listener = (payload: unknown) =>
      cb(payload as { sessionId: string; data?: string; dataB64?: string })
    subscribeEvent('ssh:data', listener)
    return () => unsubscribeEvent('ssh:data', listener)
  },
  onClosed: (cb: (payload: { sessionId: string }) => void) => {
    const listener = (payload: unknown) => cb(payload as { sessionId: string })
    subscribeEvent('ssh:closed', listener)
    return () => unsubscribeEvent('ssh:closed', listener)
  },
  onConnectionClosed: (cb: (payload: { connectionId: string }) => void) => {
    const listener = (payload: unknown) => cb(payload as { connectionId: string })
    subscribeEvent('ssh:connectionClosed', listener)
    return () => unsubscribeEvent('ssh:connectionClosed', listener)
  },
  onError: (cb: (payload: { sessionId: string; message: string }) => void) => {
    const listener = (payload: unknown) =>
      cb(payload as { sessionId: string; message: string })
    subscribeEvent('ssh:error', listener)
    return () => unsubscribeEvent('ssh:error', listener)
  },
  onBackupProgress: (cb: (payload: BackupProgress) => void) => {
    const listener = (payload: unknown) => cb(payload as BackupProgress)
    subscribeEvent('mongo:backupProgress', listener)
    return () => unsubscribeEvent('mongo:backupProgress', listener)
  },
  onFileDownloadProgress: (cb: (payload: FileDownloadProgress) => void) => {
    const listener = (payload: unknown) => cb(payload as FileDownloadProgress)
    subscribeEvent('files:downloadProgress', listener)
    return () => unsubscribeEvent('files:downloadProgress', listener)
  },

  loadQdrantConfig: (sessionId: string): Promise<QdrantConfig> =>
    rpc('qdrant:loadConfig', sessionId),
  saveQdrantConfig: (sessionId: string, cfg: QdrantConfig): Promise<QdrantConfig> =>
    rpc('qdrant:saveConfig', sessionId, cfg),
  defaultQdrantConfig: (): Promise<QdrantConfig> =>
    rpc('qdrant:defaultConfig'),
  loadQdrantLocalSettings: (): Promise<QdrantLocalSettings> =>
    rpc('qdrant:loadLocalSettings'),
  saveQdrantLocalSettings: (input: QdrantLocalSettings): Promise<QdrantLocalSettings> =>
    rpc('qdrant:saveLocalSettings', input),
  detectLocalQdrantExe: (): Promise<string | null> =>
    rpc('qdrant:detectLocalQdrantExe'),
  pickQdrantExe: async (): Promise<string | null> => {
    const picked = await open({ multiple: false, title: '选择 qdrant.exe', filters: [{ name: 'Qdrant', extensions: ['exe'] }] })
    return typeof picked === 'string' ? picked : null
  },
  listPcQdrantBackups: (localDir?: string): Promise<LocalBackupItem[]> =>
    rpc('qdrant:listPcBackups', localDir),
  peekQdrantBackupMeta: (localPath: string): Promise<QdrantBackupMetaPeek> =>
    rpc('qdrant:peekBackupMeta', localPath),
  probeLocalQdrantStatus: (collection?: string): Promise<{
    running: boolean
    qdrantUrl: string
    dataDir: string
    pointsCount?: number
    collection?: string
    status?: string
    message: string
  }> => rpc('qdrant:probeLocalStatus', collection),
  startLocalQdrant: (collection?: string): Promise<{
    running: boolean
    qdrantUrl: string
    dataDir: string
    pointsCount?: number
    collection?: string
    status?: string
    message: string
  }> => rpc('qdrant:startLocal', collection),
  stopLocalQdrant: (): Promise<{
    running: boolean
    qdrantUrl: string
    dataDir: string
    pointsCount?: number
    collection?: string
    status?: string
    message: string
  }> => rpc('qdrant:stopLocal'),
  listQdrantCollections: (
    sessionId: string,
    cfg: QdrantConfig
  ): Promise<QdrantCollectionInfo[]> =>
    rpc('qdrant:listCollections', sessionId, cfg),
  listLocalQdrantBackups: (sessionId: string, cfg: QdrantConfig): Promise<LocalBackupItem[]> =>
    rpc('qdrant:listLocalBackups', sessionId, cfg),
  listOssQdrantBackups: (cfg: QdrantConfig): Promise<OssBackupItem[]> =>
    rpc('qdrant:listOssBackups', cfg),
  deleteLocalQdrantBackup: (sessionId: string, remotePath: string): Promise<boolean> =>
    rpc('qdrant:deleteLocalBackup', sessionId, remotePath),
  deleteOssQdrantBackup: (cfg: QdrantConfig, objectKey: string): Promise<boolean> =>
    rpc('qdrant:deleteOssBackup', cfg, objectKey),
  backupQdrant: (
    sessionId: string,
    collection: string,
    cfg: QdrantConfig
  ): Promise<{
    fileName: string
    remotePath: string
    ossKey: string
    bytes: number
    uploaded: boolean
  }> => rpc('qdrant:backup', sessionId, collection, cfg),
  downloadOssQdrant: (
    sessionId: string,
    cfg: QdrantConfig,
    objectKey: string,
    localDir?: string
  ): Promise<{ fileName: string; localPath: string; bytes: number }> =>
    rpc('qdrant:downloadOss', sessionId, cfg, objectKey, localDir),
  restoreQdrantFromLocal: (
    sessionId: string,
    localPath: string,
    cfg: QdrantConfig,
    collection?: string
  ): Promise<{
    ok: true
    collection: string
    qdrantUrl: string
    dataDir: string
    pointsCount: number
    status: string
  }> =>
    rpc('qdrant:restoreFromLocal', sessionId, localPath, cfg, collection),
  uploadServerQdrantToOss: (
    sessionId: string,
    remotePath: string,
    cfg: QdrantConfig
  ): Promise<{
    fileName: string
    remotePath: string
    ossKey: string
    bytes: number
    uploaded: boolean
    via: 'server-internal' | 'server-public' | 'local-relay'
  }> => rpc('qdrant:uploadServerToOss', sessionId, remotePath, cfg),
  onQdrantBackupProgress: (cb: (payload: QdrantBackupProgress) => void) => {
    const listener = (payload: unknown) => cb(payload as QdrantBackupProgress)
    subscribeEvent('qdrant:backupProgress', listener)
    return () => unsubscribeEvent('qdrant:backupProgress', listener)
  }
}

