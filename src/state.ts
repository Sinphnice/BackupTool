import { load, type Store } from "@tauri-apps/plugin-store";
import type {
  AppState,
  RepositoryInfo,
  RepositoryRecord,
  RepositoryWorkspace,
  WorkspaceRoute,
} from "./types";

const STORE_PATH = "backup-tool-ui.json";
const STORE_KEY = "appState";
const DEFAULT_SIDEBAR_WIDTH = 260;
let storePromise: Promise<Store> | undefined;
let saveTimer: number | undefined;

function blankFilter() {
  return {
    includePath: "",
    excludePath: "",
    extensions: "",
    includeName: "",
    excludeName: "",
    minSize: "",
    maxSize: "",
    modifiedAfter: "",
    modifiedBefore: "",
  };
}

export function createWorkspace(): RepositoryWorkspace {
  return {
    route: { kind: "overview" },
    sourcePaths: [],
    compressionAlgorithm: "none",
    encryptionAlgorithm: "none",
    snapshotTitle: "",
    filtersOpen: false,
    filter: blankFilter(),
    exportPath: "",
    restoreDestination: "",
    restorePathStrategy: "preserveRelativePath",
    flattenConflictStrategy: "rename",
  };
}

export function createState(): AppState {
  return {
    version: 1,
    sidebarWidth: DEFAULT_SIDEBAR_WIDTH,
    sidebarCollapsed: false,
    sidebarSections: {
      pinnedExpanded: true,
      repositoriesExpanded: true,
    },
    repositories: [],
    workspaces: {},
  };
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringValue(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

function routeValue(value: unknown): WorkspaceRoute {
  if (!isObject(value)) return { kind: "overview" };
  if (value.kind === "add" || value.kind === "export" || value.kind === "overview") {
    return { kind: value.kind };
  }
  if (value.kind === "restore" && typeof value.snapshotId === "string") {
    return { kind: "restore", snapshotId: value.snapshotId };
  }
  return { kind: "overview" };
}

function workspaceValue(value: unknown): RepositoryWorkspace {
  const fallback = createWorkspace();
  if (!isObject(value)) return fallback;
  const filter = isObject(value.filter) ? value.filter : {};
  const compression = value.compressionAlgorithm === "zstd" ? "zstd" : "none";
  const encryption = value.encryptionAlgorithm === "aes-256-gcm" ? "aes-256-gcm" : "none";
  const pathStrategy =
    value.restorePathStrategy === "preserveFullPath" || value.restorePathStrategy === "flatten"
      ? value.restorePathStrategy
      : "preserveRelativePath";
  const conflict = ["rename", "error", "skip", "overwrite"].includes(
    String(value.flattenConflictStrategy),
  )
    ? (value.flattenConflictStrategy as RepositoryWorkspace["flattenConflictStrategy"])
    : "rename";
  return {
    route: routeValue(value.route),
    sourcePaths: Array.isArray(value.sourcePaths)
      ? value.sourcePaths.filter((item): item is string => typeof item === "string")
      : [],
    compressionAlgorithm: compression,
    encryptionAlgorithm: encryption,
    snapshotTitle: stringValue(value.snapshotTitle),
    filtersOpen: value.filtersOpen === true,
    filter: {
      includePath: stringValue(filter.includePath),
      excludePath: stringValue(filter.excludePath),
      extensions: stringValue(filter.extensions),
      includeName: stringValue(filter.includeName),
      excludeName: stringValue(filter.excludeName),
      minSize: stringValue(filter.minSize),
      maxSize: stringValue(filter.maxSize),
      modifiedAfter: stringValue(filter.modifiedAfter),
      modifiedBefore: stringValue(filter.modifiedBefore),
    },
    exportPath: stringValue(value.exportPath),
    restoreDestination: stringValue(value.restoreDestination),
    restorePathStrategy: pathStrategy,
    flattenConflictStrategy: conflict,
  };
}

function sanitizeState(value: unknown): AppState {
  if (!isObject(value) || value.version !== 1) return createState();
  const repositories = Array.isArray(value.repositories)
    ? value.repositories.flatMap((item, index): RepositoryRecord[] => {
        if (!isObject(item) || typeof item.path !== "string" || typeof item.name !== "string") {
          return [];
        }
        return [
          {
            path: item.path,
            name: item.name,
            pinned: item.pinned === true,
            archived: item.archived === true,
            lastOpenedAt:
              typeof item.lastOpenedAt === "number" ? item.lastOpenedAt : Date.now(),
            listOrder: typeof item.listOrder === "number" ? item.listOrder : index,
          },
        ];
      })
    : [];
  const workspaces: Record<string, RepositoryWorkspace> = {};
  if (isObject(value.workspaces)) {
    for (const [path, workspace] of Object.entries(value.workspaces)) {
      workspaces[path] = workspaceValue(workspace);
    }
  }
  return {
    version: 1,
    sidebarWidth:
      typeof value.sidebarWidth === "number"
        ? Math.min(380, Math.max(220, value.sidebarWidth))
        : DEFAULT_SIDEBAR_WIDTH,
    sidebarCollapsed: value.sidebarCollapsed === true,
    sidebarSections: {
      pinnedExpanded:
        isObject(value.sidebarSections) && typeof value.sidebarSections.pinnedExpanded === "boolean"
          ? value.sidebarSections.pinnedExpanded
          : true,
      repositoriesExpanded:
        isObject(value.sidebarSections) && typeof value.sidebarSections.repositoriesExpanded === "boolean"
          ? value.sidebarSections.repositoriesExpanded
          : true,
    },
    repositories,
    activeRepositoryPath:
      typeof value.activeRepositoryPath === "string" ? value.activeRepositoryPath : undefined,
    workspaces,
  };
}

function hasTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

async function getStore(): Promise<Store> {
  storePromise ??= load(STORE_PATH, { defaults: {}, autoSave: false });
  return storePromise;
}

export async function loadState(): Promise<AppState> {
  try {
    if (!hasTauriRuntime()) {
      const stored = localStorage.getItem(STORE_KEY);
      return stored ? sanitizeState(JSON.parse(stored)) : createState();
    }
    return sanitizeState(await (await getStore()).get<unknown>(STORE_KEY));
  } catch {
    return createState();
  }
}

async function writeState(snapshot: AppState): Promise<void> {
  if (!hasTauriRuntime()) {
    localStorage.setItem(STORE_KEY, JSON.stringify(snapshot));
    return;
  }
  const store = await getStore();
  await store.set(STORE_KEY, snapshot);
  await store.save();
}

export function scheduleStateSave(state: AppState): void {
  if (saveTimer !== undefined) window.clearTimeout(saveTimer);
  const snapshot = structuredClone(state);
  saveTimer = window.setTimeout(() => {
    saveTimer = undefined;
    void writeState(snapshot);
  }, 120);
}

function pathKey(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  return /^[a-z]:\//i.test(normalized) || normalized.startsWith("//")
    ? normalized.toLocaleLowerCase()
    : normalized;
}

export function ensureWorkspace(state: AppState, path: string): RepositoryWorkspace {
  state.workspaces[path] ??= createWorkspace();
  return state.workspaces[path];
}

function nextListOrder(state: AppState, pinned: boolean): number {
  return (
    Math.max(
      -1,
      ...state.repositories
        .filter((repository) => !repository.archived && repository.pinned === pinned)
        .map((repository) => repository.listOrder),
    ) + 1
  );
}

export function upsertRepository(
  state: AppState,
  info: RepositoryInfo,
  options: { updateLastOpenedAt?: boolean } = {},
): RepositoryRecord {
  const updateLastOpenedAt = options.updateLastOpenedAt ?? true;
  const key = pathKey(info.path);
  let repository = state.repositories.find((item) => pathKey(item.path) === key);
  if (!repository) {
    repository = {
      ...info,
      pinned: false,
      archived: false,
      lastOpenedAt: Date.now(),
      listOrder: nextListOrder(state, false),
    };
    state.repositories.push(repository);
  } else {
    const oldPath = repository.path;
    repository.path = info.path;
    repository.name = info.name;
    repository.archived = false;
    if (updateLastOpenedAt) repository.lastOpenedAt = Date.now();
    if (oldPath !== info.path && state.workspaces[oldPath]) {
      state.workspaces[info.path] = state.workspaces[oldPath];
      delete state.workspaces[oldPath];
    }
  }
  ensureWorkspace(state, repository.path);
  state.activeRepositoryPath = repository.path;
  return repository;
}

export function visibleRepositories(state: AppState): RepositoryRecord[] {
  return state.repositories
    .filter((repository) => !repository.archived)
    .sort((left, right) => left.listOrder - right.listOrder || right.lastOpenedAt - left.lastOpenedAt);
}

export function visiblePinnedRepositories(state: AppState): RepositoryRecord[] {
  return visibleRepositories(state).filter((repository) => repository.pinned);
}

export function visibleUnpinnedRepositories(state: AppState): RepositoryRecord[] {
  return visibleRepositories(state).filter((repository) => !repository.pinned);
}

export function setRepositoryPinned(state: AppState, path: string, pinned: boolean): void {
  const repository = state.repositories.find((item) => item.path === path);
  if (!repository || repository.pinned === pinned) return;
  repository.pinned = pinned;
  repository.listOrder = nextListOrder(state, pinned);
}

export function reorderRepositories(state: AppState, pinned: boolean, orderedPaths: readonly string[]): void {
  const orderByPath = new Map(orderedPaths.map((path, index) => [path, index]));
  for (const repository of state.repositories) {
    const order = orderByPath.get(repository.path);
    if (order !== undefined && !repository.archived && repository.pinned === pinned) {
      repository.listOrder = order;
    }
  }
}

export function reconcileSnapshotRoute(
  workspace: RepositoryWorkspace,
  snapshotIds: ReadonlySet<string>,
): boolean {
  if (workspace.route.kind !== "restore" || snapshotIds.has(workspace.route.snapshotId)) {
    return false;
  }
  workspace.route = { kind: "overview" };
  return true;
}
