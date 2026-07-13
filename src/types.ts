export type RepositoryInfo = {
  path: string;
  name: string;
  encryptionAlgorithm: "none" | "aes-256-gcm";
};

export type RepositoryRecord = RepositoryInfo & {
  pinned: boolean;
  archived: boolean;
  lastOpenedAt: number;
  listOrder: number;
};

export type SidebarSectionState = {
  pinnedExpanded: boolean;
  repositoriesExpanded: boolean;
};

export type SnapshotInfo = {
  id: string;
  fileCount: number;
  byteCount: number;
  createdUnixSeconds?: number;
  createdNanoseconds?: number;
  sequence?: number;
  title?: string;
  hasEncryptedObjects: boolean;
};

export type BackupFilterDraft = {
  pathRegex: string;
  owner: string;
  minSize: string;
  maxSize: string;
  modifiedAfter: string;
  modifiedBefore: string;
};

export type WorkspaceRoute =
  | { kind: "overview" }
  | { kind: "add" }
  | { kind: "export" }
  | { kind: "restore"; snapshotId: string };

export type RepositoryWorkspace = {
  route: WorkspaceRoute;
  sourcePaths: string[];
  compressionAlgorithm: "none" | "zstd";
  encryptSnapshot: boolean;
  snapshotTitle: string;
  filtersOpen: boolean;
  filter: BackupFilterDraft;
  exportPath: string;
  restoreDestination: string;
  restorePathStrategy: "preserveRelativePath" | "preserveFullPath" | "flatten";
  flattenConflictStrategy: "rename" | "error" | "skip" | "overwrite";
};

export type AppState = {
  version: 1;
  sidebarWidth: number;
  windowSize: { width: number; height: number };
  sidebarCollapsed: boolean;
  sidebarSections: SidebarSectionState;
  repositories: RepositoryRecord[];
  activeRepositoryPath?: string;
  workspaces: Record<string, RepositoryWorkspace>;
};

export type BackupResult = {
  fileCount: number;
  byteCount: number;
  snapshotId: string;
  snapshotTitle?: string;
  ignoredSources: string[];
};

export type ArchiveResult = {
  algorithm: string;
  path: string;
  byteCount: number;
};

export type RestoreResult = {
  fileCount: number;
  byteCount: number;
};

export type SnapshotDeleteResult = {
  snapshotId: string;
  deletedObjectCount: number;
  reclaimedBytes: number;
  warnings: string[];
};
