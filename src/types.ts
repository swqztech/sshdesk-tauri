export type AuthType = 'password' | 'privateKey'

export type HostView = {
  id: string
  name: string
  host: string
  port: number
  username: string
  authType: AuthType
  privateKeyPath?: string
  hasPassword: boolean
  hasPassphrase: boolean
  createdAt: number
  updatedAt: number
}

export type HostInput = {
  name: string
  host: string
  port?: number
  username: string
  authType: AuthType
  password?: string
  privateKeyPath?: string
  passphrase?: string
  keepPassword?: boolean
  keepPassphrase?: boolean
}

export type SessionInfo = {
  id: string
  hostId: string
  title: string
  connected: boolean
}

export type FileBrowserTab = {
  id: string
  path: string
}

export type FileBrowserState = {
  tabs: FileBrowserTab[]
  activeTabId: string
}

export type FileDownloadProgress = {
  sessionId: string
  remotePath: string
  localPath: string
  transferred: number
  total: number
}

export type TerminalTabPersist = {
  id: string
  label: string
  cwd?: string
  initialCommand?: string
}

export type TerminalState = {
  tabs: TerminalTabPersist[]
  activeTabId: string
}

export type RemoteFileEntry = {
  name: string
  path: string
  isDirectory: boolean
  size: number
  mtimeMs: number
}

export type HostDiskStat = {
  mount: string
  sizeBytes: number
  usedBytes: number
  availBytes: number
  usePercent: number
}

export type HostStats = {
  hostname: string
  os: string
  kernel: string
  cores: number
  uptimeSec: number
  cpuPercent: number
  memTotalBytes: number
  memUsedBytes: number
  memPercent: number
  swapTotalBytes: number
  swapUsedBytes: number
  load1: number
  load5: number
  load15: number
  disks: HostDiskStat[]
  collectedAt: number
}

export type Pm2Process = {
  pmId: number
  name: string
  status: string
  uptimeSec: number
  cpuPercent: number
  memoryBytes: number
  pid: number
  restarts: number
}

export type HostPm2Snapshot = {
  processes: Pm2Process[]
  collectedAt: number
  notice?: string
}

export type MongoConn = {
  host?: string
  port?: number
  username?: string
  password?: string
  authDb?: string
}

/** 本机恢复用的 Mongo 连接（默认 127.0.0.1:27017） */
export type MongoLocalConn = {
  host: string
  port: number
  username?: string
  password?: string
  authDb?: string
}

export type MongoDbInfo = {
  name: string
  sizeOnDisk: number
  empty: boolean
}

export type BackupConfig = {
  tukuBackendRoot: string
  mongodbUrl: string
  mongoBackupDb: string
  mongoBackupDir: string
  mongoBackupRetainDays: number
  ossAccessKeyId: string
  ossAccessKeySecret: string
  ossBucket: string
  ossRegion: string
  ossEndpoint: string
  ossInternal: boolean
  ossBackupRoot: string
  ossMongodbPrefix: string
  ossMongodbRetainDays: number
  configPath: string
}

export type LocalBackupItem = {
  name: string
  path: string
  size: number
  mtimeMs: number
}

export type OssBackupItem = {
  key: string
  name: string
  size: number
  lastModified: string
}

export type BackupProgress = {
  sessionId: string
  dbName: string
  phase: 'prepare' | 'dump' | 'zip' | 'upload' | 'download' | 'restore' | 'done' | 'error'
  message: string
  percent: number
}

export type MongoBackupMeta = {
  version: 1
  type: 'full' | 'incremental'
  db: string
  backupId: string
  fileName: string
  createdAt: string
  since: string | null
  baseBackupId: string
  parentBackupId: string | null
  collections: Record<
    string,
    { count: number; maxId: string | null; updateField: string | null }
  >
  changedCollections?: string[]
}

export type QdrantLocalSettings = {
  qdrantExe: string
  qdrantDataDir: string
}

export type QdrantBackupMetaPeek = {
  collection?: string
  timestamp?: string
  snapshot?: string
}

export type QdrantConfig = {
  qdrantUrl: string
  qdrantApiKey: string
  qdrantCollection: string
  qdrantBackupDir: string
  qdrantBackupRetainDays: number
  qdrantDataDir: string
  qdrantConfigFile: string
  tukuBackendRoot: string
  ossAccessKeyId: string
  ossAccessKeySecret: string
  ossBucket: string
  ossRegion: string
  ossEndpoint: string
  ossInternal: boolean
  ossBackupRoot: string
  ossQdrantPrefix: string
  ossQdrantRetainDays: number
  configPath: string
}

export type QdrantCollectionInfo = {
  name: string
  pointsCount: number
  status: string
}

export type QdrantBackupProgress = {
  sessionId: string
  collection: string
  phase: 'prepare' | 'snapshot' | 'pack' | 'upload' | 'download' | 'restore' | 'done' | 'error'
  message: string
  percent: number
}
