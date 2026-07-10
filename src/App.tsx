import { useEffect, useMemo, useState, type ReactElement, type ReactNode } from "react";
import {
  chooseDirectory,
  chooseExportPath,
  chooseTarArchive,
  confirmSnapshotDeletion,
  repositoryApi,
} from "./api";
import { activeRepository as findActiveRepository, repositoryByPath } from "./routing";
import {
  createState,
  createWorkspace,
  ensureWorkspace,
  loadState,
  reconcileSnapshotRoute,
  scheduleStateSave,
  upsertRepository,
  visibleRepositories,
} from "./state";
import type {
  AppState,
  BackupFilterDraft,
  RepositoryRecord,
  RepositoryWorkspace,
  SnapshotInfo,
  WorkspaceRoute,
} from "./types";

type Notice = { tone: "info" | "success" | "error" | "warning"; message: string };
type GlobalPage = "new" | "import" | null;
type SnapshotMap = Record<string, SnapshotInfo[] | undefined>;

const MIN_SIDEBAR_WIDTH = 220;
const MAX_SIDEBAR_WIDTH = 380;

function noticeKey(path: string, route: WorkspaceRoute["kind"]): string {
  return `${path}\n${route}`;
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let size = value / 1024;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(size >= 10 ? 1 : 2)} ${units[unit]}`;
}

function formatSnapshotTime(snapshot: SnapshotInfo): string {
  return snapshot.createdUnixSeconds
    ? new Date(snapshot.createdUnixSeconds * 1000).toLocaleString()
    : "Unknown time";
}

function normalizePathKey(path: string): string {
  return path.replace(/\\/g, "/").toLocaleLowerCase();
}

function Icon({ name }: { name: string }): ReactElement {
  return (
    <span className="button-icon" aria-hidden="true">
      {name}
    </span>
  );
}

function NoticeView({ notice }: { notice?: Notice }): ReactElement {
  return (
    <p className="page-notice" data-tone={notice?.tone ?? "info"} hidden={!notice?.message} aria-live="polite">
      {notice?.message ?? ""}
    </p>
  );
}

function SecondaryPage({
  title,
  subtitle,
  onBack,
  children,
}: {
  title: string;
  subtitle: string;
  onBack: () => void;
  children: ReactNode;
}): ReactElement {
  return (
    <>
      <header className="page-bar">
        <button className="compact-button icon-button-text" type="button" aria-label="Back" onClick={onBack}>
          <Icon name="←" />
          <span>Back</span>
        </button>
        <div className="page-heading">
          <h1 className="page-title">{title}</h1>
          <p className="page-subtitle" title={subtitle}>
            {subtitle}
          </p>
        </div>
      </header>
      <div className="page-content">{children}</div>
    </>
  );
}

function Sidebar({
  state,
  unavailable,
  globalPageActive,
  onNew,
  onOpen,
  onImport,
  onActivate,
  onTogglePin,
  onArchive,
}: {
  state: AppState;
  unavailable: ReadonlySet<string>;
  globalPageActive: boolean;
  onNew: () => void;
  onOpen: () => void;
  onImport: () => void;
  onActivate: (path: string) => void;
  onTogglePin: (path: string) => void;
  onArchive: (path: string) => void;
}): ReactElement {
  const repositories = visibleRepositories(state);
  return (
    <aside id="repository-sidebar" aria-label="Repositories">
      <header className="sidebar-header">
        <h1>BackupTool</h1>
      </header>
      <div className="sidebar-actions">
        <button className="sidebar-action" type="button" onClick={onNew} title="New repository">
          <Icon name="+" />
          <span>New</span>
        </button>
        <button className="sidebar-action" type="button" onClick={onOpen} title="Open repository">
          <Icon name="□" />
          <span>Open</span>
        </button>
        <button className="sidebar-action" type="button" onClick={onImport} title="Import repository">
          <Icon name="⇩" />
          <span>Import</span>
        </button>
      </div>
      <div className="sidebar-section-title">Repositories</div>
      <div className="repository-list" id="repository-list">
        {repositories.length === 0 ? <p className="sidebar-empty">No repositories opened</p> : null}
        {repositories.map((repository) => (
          <div
            className={[
              "repository-row",
              !globalPageActive && repository.path === state.activeRepositoryPath ? "is-active" : "",
              unavailable.has(repository.path) ? "is-unavailable" : "",
            ]
              .filter(Boolean)
              .join(" ")}
            key={repository.path}
            tabIndex={0}
            title={repository.path}
            onClick={(event) => {
              if (!(event.target instanceof HTMLButtonElement)) onActivate(repository.path);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                onActivate(repository.path);
              }
            }}
          >
            <div className="repository-details">
              <span className="repository-name">{repository.name}</span>
              <span className="repository-path">{unavailable.has(repository.path) ? "Unavailable" : repository.path}</span>
            </div>
            <div className="repository-actions">
              <button
                type="button"
                className="row-action icon-button"
                title={repository.pinned ? "Unpin repository" : "Pin repository"}
                onClick={(event) => {
                  event.stopPropagation();
                  onTogglePin(repository.path);
                }}
              >
                <Icon name={repository.pinned ? "⌂" : "⌃"} />
              </button>
              <button
                type="button"
                className="row-action icon-button"
                title="Archive repository"
                onClick={(event) => {
                  event.stopPropagation();
                  onArchive(repository.path);
                }}
              >
                <Icon name="×" />
              </button>
            </div>
          </div>
        ))}
      </div>
    </aside>
  );
}

function EmptyWorkspace(): ReactElement {
  return (
    <div className="empty-workspace">
      <div>
        <p className="eyebrow">Repository workspace</p>
        <h1>Open a backup repository</h1>
        <p>Select an existing repository, create a new one, or import a tar archive.</p>
      </div>
    </div>
  );
}

function NewRepositoryPage({
  notice,
  busy,
  onBack,
  onBrowse,
  onSubmit,
}: {
  notice?: Notice;
  busy: boolean;
  onBack: () => void;
  onBrowse: () => Promise<string | undefined>;
  onSubmit: (parentPath: string, name: string) => void;
}): ReactElement {
  const [parentPath, setParentPath] = useState("");
  const [name, setName] = useState("");
  return (
    <SecondaryPage title="New Repository" subtitle="Create a repository directory in a selected location" onBack={onBack}>
      <form
        className="form-panel"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(parentPath, name);
        }}
      >
        <label>
          Parent directory
          <span className="path-control">
            <input value={parentPath} onChange={(event) => setParentPath(event.target.value)} disabled={busy} autoComplete="off" />
            <button
              type="button"
              className="secondary-button icon-button-text"
              disabled={busy}
              onClick={async () => {
                const selected = await onBrowse();
                if (selected) setParentPath(selected);
              }}
            >
              <Icon name="□" />
              <span>Browse</span>
            </button>
          </span>
        </label>
        <label>
          Repository name
          <input value={name} onChange={(event) => setName(event.target.value)} disabled={busy} autoComplete="off" maxLength={120} />
        </label>
        <div className="form-actions">
          <button type="submit" className="primary-button" disabled={busy}>
            Create Repository
          </button>
        </div>
        <NoticeView notice={notice} />
      </form>
    </SecondaryPage>
  );
}

function ImportRepositoryPage({
  notice,
  busy,
  onBack,
  onBrowseArchive,
  onBrowseDestination,
  onSubmit,
}: {
  notice?: Notice;
  busy: boolean;
  onBack: () => void;
  onBrowseArchive: () => Promise<string | undefined>;
  onBrowseDestination: () => Promise<string | undefined>;
  onSubmit: (archivePath: string, destination: string) => void;
}): ReactElement {
  const [archivePath, setArchivePath] = useState("");
  const [destination, setDestination] = useState("");
  return (
    <SecondaryPage title="Import Repository" subtitle="Import a tar archive into an empty directory" onBack={onBack}>
      <form
        className="form-panel"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(archivePath, destination);
        }}
      >
        <label>
          Archive algorithm
          <select disabled>
            <option value="tar">tar</option>
          </select>
        </label>
        <label>
          Archive file
          <span className="path-control">
            <input value={archivePath} onChange={(event) => setArchivePath(event.target.value)} disabled={busy} autoComplete="off" />
            <button
              type="button"
              className="secondary-button icon-button-text"
              disabled={busy}
              onClick={async () => {
                const selected = await onBrowseArchive();
                if (selected) setArchivePath(selected);
              }}
            >
              <Icon name="□" />
              <span>Browse</span>
            </button>
          </span>
        </label>
        <label>
          Destination directory
          <span className="path-control">
            <input value={destination} onChange={(event) => setDestination(event.target.value)} disabled={busy} autoComplete="off" />
            <button
              type="button"
              className="secondary-button icon-button-text"
              disabled={busy}
              onClick={async () => {
                const selected = await onBrowseDestination();
                if (selected) setDestination(selected);
              }}
            >
              <Icon name="□" />
              <span>Browse</span>
            </button>
          </span>
        </label>
        <div className="form-actions">
          <button type="submit" className="primary-button" disabled={busy}>
            Import Repository
          </button>
        </div>
        <NoticeView notice={notice} />
      </form>
    </SecondaryPage>
  );
}

function OverviewPage({
  repository,
  snapshots,
  notice,
  busySnapshotId,
  onAdd,
  onExport,
  onRefresh,
  onRestore,
  onDelete,
}: {
  repository: RepositoryRecord;
  snapshots: SnapshotInfo[] | undefined;
  notice?: Notice;
  busySnapshotId?: string;
  onAdd: () => void;
  onExport: () => void;
  onRefresh: () => void;
  onRestore: (snapshotId: string) => void;
  onDelete: (snapshot: SnapshotInfo) => void;
}): ReactElement {
  return (
    <>
      <header className="workspace-header">
        <div className="workspace-identity">
          <p className="eyebrow">Repository</p>
          <h1>{repository.name}</h1>
          <p className="workspace-path" title={repository.path}>{repository.path}</p>
        </div>
        <div className="workspace-actions">
          <button type="button" className="primary-button icon-button-text" onClick={onAdd}>
            <Icon name="+" />
            <span>Add Snapshot</span>
          </button>
          <button type="button" className="secondary-button icon-button-text" onClick={onExport}>
            <Icon name="⇧" />
            <span>Export Repository</span>
          </button>
        </div>
      </header>
      <div className="overview-content">
        <div className="section-heading">
          <div>
            <p className="eyebrow">History</p>
            <h2>Snapshots</h2>
          </div>
          <button type="button" className="compact-button icon-button-text" onClick={onRefresh}>
            <Icon name="↻" />
            <span>Refresh</span>
          </button>
        </div>
        <NoticeView notice={notice} />
        <div className="snapshot-list">
          {!snapshots ? <p className="snapshot-empty">Loading snapshots...</p> : null}
          {snapshots?.length === 0 ? <p className="snapshot-empty">No snapshots in this repository</p> : null}
          {snapshots?.map((snapshot) => (
            <article
              className="snapshot-row"
              key={snapshot.id}
              tabIndex={0}
              onClick={(event) => {
                if (!(event.target instanceof HTMLButtonElement)) onRestore(snapshot.id);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  onRestore(snapshot.id);
                }
              }}
            >
              <div className="snapshot-details">
                <h3>{snapshot.title?.trim() || "Untitled"}</h3>
                <p>
                  {formatSnapshotTime(snapshot)} / {snapshot.fileCount} files / {formatBytes(snapshot.byteCount)}
                </p>
                <code>{snapshot.id}</code>
              </div>
              <button
                type="button"
                className="danger-button snapshot-delete icon-button"
                title="Delete snapshot"
                disabled={busySnapshotId === snapshot.id}
                onClick={(event) => {
                  event.stopPropagation();
                  onDelete(snapshot);
                }}
              >
                <Icon name="×" />
              </button>
            </article>
          ))}
        </div>
      </div>
    </>
  );
}

function SourceList({
  sourcePaths,
  onRemove,
}: {
  sourcePaths: readonly string[];
  onRemove: (index: number) => void;
}): ReactElement {
  if (sourcePaths.length === 0) {
    return (
      <div className="source-list">
        <p className="source-empty">No source directories added</p>
      </div>
    );
  }
  return (
    <div className="source-list">
      {sourcePaths.map((source, index) => (
        <div className="source-row" key={`${source}-${index}`}>
          <span title={source}>{source}</span>
          <button type="button" className="compact-button icon-button" title="Remove source" onClick={() => onRemove(index)}>
            <Icon name="×" />
          </button>
        </div>
      ))}
    </div>
  );
}

function AddSnapshotPage({
  repository,
  draft,
  notice,
  busy,
  encryptionPassword,
  onBack,
  onBrowseSource,
  onChangeDraft,
  onChangeFilter,
  onRemoveSource,
  onPasswordChange,
  onSubmit,
}: {
  repository: RepositoryRecord;
  draft: RepositoryWorkspace;
  notice?: Notice;
  busy: boolean;
  encryptionPassword: string;
  onBack: () => void;
  onBrowseSource: () => void;
  onChangeDraft: <K extends keyof RepositoryWorkspace>(field: K, value: RepositoryWorkspace[K]) => void;
  onChangeFilter: <K extends keyof BackupFilterDraft>(field: K, value: BackupFilterDraft[K]) => void;
  onRemoveSource: (index: number) => void;
  onPasswordChange: (value: string) => void;
  onSubmit: () => void;
}): ReactElement {
  return (
    <SecondaryPage title="Add Snapshot" subtitle={repository.path} onBack={onBack}>
      <form
        className="form-panel wide-form"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit();
        }}
      >
        <div className="field-group">
          <div className="group-heading">
            <div>
              <h2>Source directories</h2>
              <p>Add one or more directories to this snapshot.</p>
            </div>
            <button type="button" className="secondary-button icon-button-text" onClick={onBrowseSource} disabled={busy}>
              <Icon name="+" />
              <span>Add</span>
            </button>
          </div>
          <SourceList sourcePaths={draft.sourcePaths} onRemove={onRemoveSource} />
        </div>
        <div className="form-grid two-columns">
          <label>
            Compression algorithm
            <select
              value={draft.compressionAlgorithm}
              disabled={busy}
              onChange={(event) => onChangeDraft("compressionAlgorithm", event.target.value === "zstd" ? "zstd" : "none")}
            >
              <option value="none">None</option>
              <option value="zstd">zstd</option>
            </select>
          </label>
          <label>
            Encryption algorithm
            <select
              value={draft.encryptionAlgorithm}
              disabled={busy}
              onChange={(event) => {
                const value = event.target.value === "aes-256-gcm" ? "aes-256-gcm" : "none";
                onChangeDraft("encryptionAlgorithm", value);
                if (value === "none") onPasswordChange("");
              }}
            >
              <option value="none">None</option>
              <option value="aes-256-gcm">AES-256-GCM</option>
            </select>
          </label>
          {draft.encryptionAlgorithm === "aes-256-gcm" ? (
            <label>
              Encryption password
              <input value={encryptionPassword} onChange={(event) => onPasswordChange(event.target.value)} disabled={busy} type="password" autoComplete="new-password" />
            </label>
          ) : null}
          <label>
            Snapshot title
            <input value={draft.snapshotTitle} disabled={busy} onChange={(event) => onChangeDraft("snapshotTitle", event.target.value)} type="text" maxLength={120} autoComplete="off" />
          </label>
        </div>
        <details className="advanced-panel" open={draft.filtersOpen} onToggle={(event) => onChangeDraft("filtersOpen", event.currentTarget.open)}>
          <summary>Advanced filters</summary>
          <div className="form-grid three-columns">
            <label>Include path contains<input value={draft.filter.includePath} disabled={busy} onChange={(event) => onChangeFilter("includePath", event.target.value)} autoComplete="off" /></label>
            <label>Exclude path contains<input value={draft.filter.excludePath} disabled={busy} onChange={(event) => onChangeFilter("excludePath", event.target.value)} autoComplete="off" /></label>
            <label>Extensions<input value={draft.filter.extensions} disabled={busy} onChange={(event) => onChangeFilter("extensions", event.target.value)} autoComplete="off" placeholder="txt;png" /></label>
            <label>Include file name contains<input value={draft.filter.includeName} disabled={busy} onChange={(event) => onChangeFilter("includeName", event.target.value)} autoComplete="off" /></label>
            <label>Exclude file name contains<input value={draft.filter.excludeName} disabled={busy} onChange={(event) => onChangeFilter("excludeName", event.target.value)} autoComplete="off" /></label>
            <label>Minimum size<input value={draft.filter.minSize} disabled={busy} onChange={(event) => onChangeFilter("minSize", event.target.value)} type="number" min="0" step="1" /></label>
            <label>Maximum size<input value={draft.filter.maxSize} disabled={busy} onChange={(event) => onChangeFilter("maxSize", event.target.value)} type="number" min="0" step="1" /></label>
            <label>Modified after<input value={draft.filter.modifiedAfter} disabled={busy} onChange={(event) => onChangeFilter("modifiedAfter", event.target.value)} type="datetime-local" /></label>
            <label>Modified before<input value={draft.filter.modifiedBefore} disabled={busy} onChange={(event) => onChangeFilter("modifiedBefore", event.target.value)} type="datetime-local" /></label>
          </div>
        </details>
        <div className="form-actions">
          <button type="submit" className="primary-button" disabled={busy}>
            Add Snapshot
          </button>
        </div>
        <NoticeView notice={notice} />
      </form>
    </SecondaryPage>
  );
}

function ExportRepositoryPage({
  repository,
  draft,
  notice,
  busy,
  onBack,
  onChangeExportPath,
  onBrowse,
  onSubmit,
}: {
  repository: RepositoryRecord;
  draft: RepositoryWorkspace;
  notice?: Notice;
  busy: boolean;
  onBack: () => void;
  onChangeExportPath: (value: string) => void;
  onBrowse: () => void;
  onSubmit: () => void;
}): ReactElement {
  return (
    <SecondaryPage title="Export Repository" subtitle={repository.path} onBack={onBack}>
      <form
        className="form-panel"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit();
        }}
      >
        <label>
          Archive algorithm
          <select disabled>
            <option value="tar">tar</option>
          </select>
        </label>
        <label>
          Export file
          <span className="path-control">
            <input value={draft.exportPath} disabled={busy} onChange={(event) => onChangeExportPath(event.target.value)} autoComplete="off" />
            <button type="button" className="secondary-button icon-button-text" disabled={busy} onClick={onBrowse}>
              <Icon name="□" />
              <span>Browse</span>
            </button>
          </span>
        </label>
        <div className="form-actions">
          <button type="submit" className="primary-button" disabled={busy}>
            Export Repository
          </button>
        </div>
        <NoticeView notice={notice} />
      </form>
    </SecondaryPage>
  );
}

function RestoreSnapshotPage({
  repository,
  snapshot,
  draft,
  notice,
  busy,
  decryptionPassword,
  onBack,
  onChangeDraft,
  onPasswordChange,
  onBrowse,
  onSubmit,
}: {
  repository: RepositoryRecord;
  snapshot: SnapshotInfo;
  draft: RepositoryWorkspace;
  notice?: Notice;
  busy: boolean;
  decryptionPassword: string;
  onBack: () => void;
  onChangeDraft: <K extends keyof RepositoryWorkspace>(field: K, value: RepositoryWorkspace[K]) => void;
  onPasswordChange: (value: string) => void;
  onBrowse: () => void;
  onSubmit: () => void;
}): ReactElement {
  return (
    <SecondaryPage title="Restore Snapshot" subtitle={snapshot.title?.trim() || "Untitled"} onBack={onBack}>
      <form
        className="form-panel"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit();
        }}
      >
        <div className="snapshot-summary">
          <span>Repository</span><strong>{repository.name}</strong>
          <span>Created</span><strong>{formatSnapshotTime(snapshot)}</strong>
          <span>Snapshot ID</span><code>{snapshot.id}</code>
        </div>
        <label>
          Restore directory
          <span className="path-control">
            <input value={draft.restoreDestination} disabled={busy} onChange={(event) => onChangeDraft("restoreDestination", event.target.value)} autoComplete="off" />
            <button type="button" className="secondary-button icon-button-text" disabled={busy} onClick={onBrowse}>
              <Icon name="□" />
              <span>Browse</span>
            </button>
          </span>
        </label>
        <label>
          Restore path strategy
          <select value={draft.restorePathStrategy} disabled={busy} onChange={(event) => onChangeDraft("restorePathStrategy", event.target.value as RepositoryWorkspace["restorePathStrategy"])}>
            <option value="preserveRelativePath">Preserve Relative Path</option>
            <option value="preserveFullPath">Preserve Full Path</option>
            <option value="flatten">Flatten</option>
          </select>
        </label>
        {draft.restorePathStrategy === "flatten" ? (
          <label>
            Flatten conflict strategy
            <select value={draft.flattenConflictStrategy} disabled={busy} onChange={(event) => onChangeDraft("flattenConflictStrategy", event.target.value as RepositoryWorkspace["flattenConflictStrategy"])}>
              <option value="rename">Rename</option>
              <option value="error">Error</option>
              <option value="skip">Skip</option>
              <option value="overwrite">Overwrite</option>
            </select>
          </label>
        ) : null}
        <label>
          Decryption password
          <input value={decryptionPassword} disabled={busy} onChange={(event) => onPasswordChange(event.target.value)} type="password" autoComplete="current-password" />
        </label>
        <div className="form-actions">
          <button type="submit" className="primary-button" disabled={busy}>
            Restore Snapshot
          </button>
        </div>
        <NoticeView notice={notice} />
      </form>
    </SecondaryPage>
  );
}

function validateBackupDraft(draft: RepositoryWorkspace, password: string): string | undefined {
  if (draft.sourcePaths.length === 0) return "Add at least one source directory.";
  if (draft.encryptionAlgorithm === "aes-256-gcm" && !password) return "Encryption password must not be empty.";
  const minimum = draft.filter.minSize ? Number(draft.filter.minSize) : undefined;
  const maximum = draft.filter.maxSize ? Number(draft.filter.maxSize) : undefined;
  if (minimum !== undefined && maximum !== undefined && minimum > maximum) return "Minimum size must not exceed maximum size.";
  if (
    draft.filter.modifiedAfter &&
    draft.filter.modifiedBefore &&
    new Date(draft.filter.modifiedAfter) > new Date(draft.filter.modifiedBefore)
  ) {
    return "Modified after must not be later than modified before.";
  }
  return undefined;
}

export function App(): ReactElement {
  const [state, setState] = useState<AppState | null>(null);
  const [globalPage, setGlobalPage] = useState<GlobalPage>(null);
  const [globalNotice, setGlobalNotice] = useState<Notice | undefined>();
  const [notices, setNotices] = useState<Record<string, Notice>>({});
  const [snapshots, setSnapshots] = useState<SnapshotMap>({});
  const [unavailable, setUnavailable] = useState<Set<string>>(() => new Set());
  const [isResizing, setIsResizing] = useState(false);
  const [busy, setBusy] = useState<string | undefined>();
  const [busySnapshotId, setBusySnapshotId] = useState<string | undefined>();
  const [encryptionPassword, setEncryptionPassword] = useState("");
  const [decryptionPassword, setDecryptionPassword] = useState("");

  useEffect(() => {
    let cancelled = false;
    async function bootstrap(): Promise<void> {
      const loaded = await loadState();
      if ("__TAURI_INTERNALS__" in window) {
        await Promise.all(
          loaded.repositories
            .filter((repository) => !repository.archived)
            .map(async (repository) => {
              try {
                const info = await repositoryApi.open(repository.path);
                repository.name = info.name;
              } catch {
                setUnavailable((current) => new Set(current).add(repository.path));
              }
            }),
        );
      }
      const active = findActiveRepository(loaded);
      if (active && !active.archived) {
        try {
          const list = await repositoryApi.listSnapshots(active.path);
          if (!cancelled) setSnapshots((current) => ({ ...current, [active.path]: list }));
        } catch {
          if (!cancelled) setUnavailable((current) => new Set(current).add(active.path));
        }
      }
      if (!cancelled) setState(loaded);
    }
    void bootstrap().catch((error) => {
      setGlobalNotice({ tone: "error", message: `BackupTool failed to start: ${String(error)}` });
      setState(createState());
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (state) document.documentElement.style.setProperty("--sidebar-width", `${state.sidebarWidth}px`);
  }, [state?.sidebarWidth]);

  const active = state ? findActiveRepository(state) : undefined;
  const workspace = useMemo(() => {
    if (!state || !active) return undefined;
    return state.workspaces[active.path] ?? createWorkspace();
  }, [state, active]);

  function updateState(mutator: (draft: AppState) => void, persist = true): AppState | undefined {
    let nextState: AppState | undefined;
    setState((current) => {
      if (!current) return current;
      const next = structuredClone(current);
      mutator(next);
      nextState = next;
      if (persist) scheduleStateSave(next);
      return next;
    });
    return nextState;
  }

  function setRoute(route: WorkspaceRoute): void {
    updateState((draft) => {
      const repository = findActiveRepository(draft);
      if (repository) ensureWorkspace(draft, repository.path).route = route;
    });
    setEncryptionPassword("");
    setDecryptionPassword("");
  }

  function setNotice(path: string, route: WorkspaceRoute["kind"], notice: Notice): void {
    setNotices((current) => ({ ...current, [noticeKey(path, route)]: notice }));
  }

  function updateWorkspace(path: string, mutator: (workspace: RepositoryWorkspace) => void): void {
    updateState((draft) => {
      const target = ensureWorkspace(draft, path);
      mutator(target);
    });
  }

  async function refreshSnapshots(path: string, announce = true): Promise<void> {
    try {
      const list = await repositoryApi.listSnapshots(path);
      setSnapshots((current) => ({ ...current, [path]: list }));
      setUnavailable((current) => {
        const next = new Set(current);
        next.delete(path);
        return next;
      });
      updateState((draft) => {
        const target = ensureWorkspace(draft, path);
        reconcileSnapshotRoute(target, new Set(list.map((item) => item.id)));
      });
      if (announce) setNotice(path, "overview", { tone: "success", message: `Loaded ${list.length} snapshot${list.length === 1 ? "" : "s"}.` });
    } catch (error) {
      setUnavailable((current) => new Set(current).add(path));
      setNotice(path, "overview", { tone: "error", message: String(error) });
    }
  }

  async function openAndActivateRepository(path: string): Promise<void> {
    try {
      const info = await repositoryApi.open(path);
      let openedPath = info.path;
      updateState((draft) => {
        openedPath = upsertRepository(draft, info).path;
      });
      setUnavailable((current) => {
        const next = new Set(current);
        next.delete(path);
        next.delete(openedPath);
        return next;
      });
      setGlobalPage(null);
      setGlobalNotice(undefined);
      await refreshSnapshots(openedPath, false);
    } catch (error) {
      setUnavailable((current) => new Set(current).add(path));
      const repository = state ? findActiveRepository(state) : undefined;
      if (repository) setNotice(repository.path, "overview", { tone: "error", message: String(error) });
      else setGlobalNotice({ tone: "error", message: String(error) });
    }
  }

  function changeActiveRepository(path: string): void {
    void openAndActivateRepository(path);
  }

  function togglePin(path: string): void {
    updateState((draft) => {
      const repository = repositoryByPath(draft, path);
      if (repository) repository.pinned = !repository.pinned;
    });
  }

  function archiveRepository(path: string): void {
    updateState((draft) => {
      const repository = repositoryByPath(draft, path);
      if (!repository) return;
      repository.archived = true;
      if (draft.activeRepositoryPath === path) draft.activeRepositoryPath = undefined;
    });
    setGlobalPage(null);
  }

  function updateSidebarWidth(value: number): void {
    updateState((draft) => {
      draft.sidebarWidth = Math.min(MAX_SIDEBAR_WIDTH, Math.max(MIN_SIDEBAR_WIDTH, value));
    });
  }

  if (!state) {
    return (
      <main id="app-shell">
        <section id="workspace" aria-label="Workspace">
          <EmptyWorkspace />
        </section>
      </main>
    );
  }

  function workspaceNotice(route: WorkspaceRoute["kind"]): Notice | undefined {
    return active ? notices[noticeKey(active.path, route)] : undefined;
  }

  function closeGlobalPage(): void {
    setGlobalPage(null);
    setGlobalNotice(undefined);
  }

  const workspaceContent = (() => {
    if (globalPage === "new") {
      return (
        <NewRepositoryPage
          notice={globalNotice}
          busy={busy === "new"}
          onBack={closeGlobalPage}
          onBrowse={() => chooseDirectory("Choose repository parent")}
          onSubmit={async (parentPath, name) => {
            setBusy("new");
            setGlobalNotice({ tone: "info", message: "Creating repository..." });
            try {
              const info = await repositoryApi.create(parentPath, name);
              let openedPath = info.path;
              updateState((draft) => {
                openedPath = upsertRepository(draft, info).path;
              });
              setGlobalPage(null);
              setGlobalNotice(undefined);
              setNotice(openedPath, "overview", { tone: "success", message: "Repository created." });
              await refreshSnapshots(openedPath, false);
            } catch (error) {
              setGlobalNotice({ tone: "error", message: String(error) });
            } finally {
              setBusy(undefined);
            }
          }}
        />
      );
    }
    if (globalPage === "import") {
      return (
        <ImportRepositoryPage
          notice={globalNotice}
          busy={busy === "import"}
          onBack={closeGlobalPage}
          onBrowseArchive={chooseTarArchive}
          onBrowseDestination={() => chooseDirectory("Choose import destination")}
          onSubmit={async (archivePath, destination) => {
            setBusy("import");
            setGlobalNotice({ tone: "info", message: "Importing repository..." });
            try {
              const result = await repositoryApi.import(archivePath, destination);
              const info = await repositoryApi.open(result.path);
              let openedPath = info.path;
              updateState((draft) => {
                openedPath = upsertRepository(draft, info).path;
              });
              setGlobalPage(null);
              setGlobalNotice(undefined);
              setNotice(openedPath, "overview", { tone: "success", message: `Repository imported (${formatBytes(result.byteCount)}).` });
              await refreshSnapshots(openedPath, false);
            } catch (error) {
              setGlobalNotice({ tone: "error", message: String(error) });
            } finally {
              setBusy(undefined);
            }
          }}
        />
      );
    }
    if (!active || active.archived || !workspace) return <EmptyWorkspace />;

    if (workspace.route.kind === "add") {
      return (
        <AddSnapshotPage
          repository={active}
          draft={workspace}
          notice={workspaceNotice("add")}
          busy={busy === "backup"}
          encryptionPassword={encryptionPassword}
          onBack={() => setRoute({ kind: "overview" })}
          onBrowseSource={async () => {
            const selected = await chooseDirectory("Add source directory");
            if (!selected) return;
            updateWorkspace(active.path, (draft) => {
              const key = normalizePathKey(selected);
              if (!draft.sourcePaths.some((source) => normalizePathKey(source) === key)) draft.sourcePaths.push(selected);
            });
          }}
          onChangeDraft={(field, value) => updateWorkspace(active.path, (draft) => {
            draft[field] = value;
          })}
          onChangeFilter={(field, value) => updateWorkspace(active.path, (draft) => {
            draft.filter[field] = value;
          })}
          onRemoveSource={(index) => updateWorkspace(active.path, (draft) => {
            draft.sourcePaths.splice(index, 1);
          })}
          onPasswordChange={setEncryptionPassword}
          onSubmit={async () => {
            const validation = validateBackupDraft(workspace, encryptionPassword);
            if (validation) {
              setNotice(active.path, "add", { tone: "error", message: validation });
              return;
            }
            setBusy("backup");
            setNotice(active.path, "add", { tone: "info", message: "Adding snapshot..." });
            try {
              const result = await repositoryApi.backup({
                repositoryPath: active.path,
                sources: workspace.sourcePaths,
                filter: workspace.filter,
                compressionAlgorithm: workspace.compressionAlgorithm,
                encryptionAlgorithm: workspace.encryptionAlgorithm,
                encryptionPassword,
                snapshotTitle: workspace.snapshotTitle,
              });
              updateWorkspace(active.path, (draft) => {
                draft.snapshotTitle = "";
                draft.route = { kind: "overview" };
              });
              setEncryptionPassword("");
              setNotice(active.path, "overview", {
                tone: result.ignoredSources.length > 0 ? "warning" : "success",
                message: `Snapshot added: ${result.fileCount} files, ${formatBytes(result.byteCount)}.${
                  result.ignoredSources.length > 0 ? ` Ignored ${result.ignoredSources.length} duplicate or nested sources.` : ""
                }`,
              });
              await refreshSnapshots(active.path, false);
            } catch (error) {
              setNotice(active.path, "add", { tone: "error", message: String(error) });
            } finally {
              setBusy(undefined);
            }
          }}
        />
      );
    }

    if (workspace.route.kind === "export") {
      return (
        <ExportRepositoryPage
          repository={active}
          draft={workspace}
          notice={workspaceNotice("export")}
          busy={busy === "export"}
          onBack={() => setRoute({ kind: "overview" })}
          onChangeExportPath={(value) => updateWorkspace(active.path, (draft) => {
            draft.exportPath = value;
          })}
          onBrowse={async () => {
            const selected = await chooseExportPath(`${active.name}.tar`);
            if (selected) updateWorkspace(active.path, (draft) => {
              draft.exportPath = selected;
            });
          }}
          onSubmit={async () => {
            setBusy("export");
            setNotice(active.path, "export", { tone: "info", message: "Exporting repository..." });
            try {
              const result = await repositoryApi.export(active.path, workspace.exportPath);
              setNotice(active.path, "export", { tone: "success", message: `Repository exported to ${result.path} (${formatBytes(result.byteCount)}).` });
            } catch (error) {
              setNotice(active.path, "export", { tone: "error", message: String(error) });
            } finally {
              setBusy(undefined);
            }
          }}
        />
      );
    }

    if (workspace.route.kind === "restore") {
      const restoreRoute = workspace.route;
      const snapshot = snapshots[active.path]?.find((item) => item.id === restoreRoute.snapshotId);
      if (!snapshot) return <OverviewPage repository={active} snapshots={snapshots[active.path]} notice={workspaceNotice("overview")} onAdd={() => setRoute({ kind: "add" })} onExport={() => setRoute({ kind: "export" })} onRefresh={() => void refreshSnapshots(active.path)} onRestore={(snapshotId) => setRoute({ kind: "restore", snapshotId })} onDelete={() => undefined} />;
      return (
        <RestoreSnapshotPage
          repository={active}
          snapshot={snapshot}
          draft={workspace}
          notice={workspaceNotice("restore")}
          busy={busy === "restore"}
          decryptionPassword={decryptionPassword}
          onBack={() => setRoute({ kind: "overview" })}
          onChangeDraft={(field, value) => updateWorkspace(active.path, (draft) => {
            draft[field] = value;
          })}
          onPasswordChange={setDecryptionPassword}
          onBrowse={async () => {
            const selected = await chooseDirectory("Choose restore destination");
            if (selected) updateWorkspace(active.path, (draft) => {
              draft.restoreDestination = selected;
            });
          }}
          onSubmit={async () => {
            setBusy("restore");
            setNotice(active.path, "restore", { tone: "info", message: "Restoring snapshot..." });
            try {
              const result = await repositoryApi.restore({
                repositoryPath: active.path,
                snapshotId: snapshot.id,
                destination: workspace.restoreDestination,
                pathStrategy: workspace.restorePathStrategy,
                flattenConflictStrategy: workspace.flattenConflictStrategy,
                decryptionPassword,
              });
              setNotice(active.path, "restore", { tone: "success", message: `Restored ${result.fileCount} files (${formatBytes(result.byteCount)}).` });
            } catch (error) {
              setNotice(active.path, "restore", { tone: "error", message: String(error) });
            } finally {
              setBusy(undefined);
            }
          }}
        />
      );
    }

    return (
      <OverviewPage
        repository={active}
        snapshots={snapshots[active.path]}
        notice={workspaceNotice("overview")}
        busySnapshotId={busySnapshotId}
        onAdd={() => setRoute({ kind: "add" })}
        onExport={() => setRoute({ kind: "export" })}
        onRefresh={() => void refreshSnapshots(active.path)}
        onRestore={(snapshotId) => setRoute({ kind: "restore", snapshotId })}
        onDelete={async (snapshot) => {
          setBusySnapshotId(snapshot.id);
          try {
            const confirmed = await confirmSnapshotDeletion(snapshot.title?.trim() || "Untitled");
            if (!confirmed) return;
            setNotice(active.path, "overview", { tone: "info", message: "Deleting snapshot..." });
            const result = await repositoryApi.deleteSnapshot(active.path, snapshot.id);
            const warning = result.warnings.length > 0 ? ` ${result.warnings.join(" ")}` : "";
            setNotice(active.path, "overview", {
              tone: result.warnings.length > 0 ? "warning" : "success",
              message: `Snapshot deleted. Removed ${result.deletedObjectCount} objects and reclaimed ${formatBytes(result.reclaimedBytes)}.${warning}`,
            });
            await refreshSnapshots(active.path, false);
          } catch (error) {
            setNotice(active.path, "overview", { tone: "error", message: String(error) });
          } finally {
            setBusySnapshotId(undefined);
          }
        }}
      />
    );
  })();

  return (
    <main id="app-shell" className={isResizing ? "is-resizing" : undefined}>
      <Sidebar
        state={state}
        unavailable={unavailable}
        globalPageActive={globalPage !== null}
        onNew={() => {
          setGlobalPage("new");
          setGlobalNotice(undefined);
        }}
        onOpen={async () => {
          const selected = await chooseDirectory("Open repository");
          if (selected) await openAndActivateRepository(selected);
        }}
        onImport={() => {
          setGlobalPage("import");
          setGlobalNotice(undefined);
        }}
        onActivate={changeActiveRepository}
        onTogglePin={togglePin}
        onArchive={archiveRepository}
      />
      <div
        id="sidebar-resizer"
        role="separator"
        aria-label="Resize repository sidebar"
        aria-orientation="vertical"
        aria-valuemin={MIN_SIDEBAR_WIDTH}
        aria-valuemax={MAX_SIDEBAR_WIDTH}
        aria-valuenow={state.sidebarWidth}
        tabIndex={0}
        onPointerDown={(event) => {
          event.currentTarget.setPointerCapture(event.pointerId);
          setIsResizing(true);
        }}
        onPointerMove={(event) => {
          if (event.currentTarget.hasPointerCapture(event.pointerId)) updateSidebarWidth(event.clientX);
        }}
        onPointerUp={(event) => {
          if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
          setIsResizing(false);
        }}
        onPointerCancel={(event) => {
          if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
          setIsResizing(false);
        }}
        onKeyDown={(event) => {
          if (event.key === "ArrowLeft") updateSidebarWidth(state.sidebarWidth - 10);
          else if (event.key === "ArrowRight") updateSidebarWidth(state.sidebarWidth + 10);
          else if (event.key === "Home") updateSidebarWidth(MIN_SIDEBAR_WIDTH);
          else if (event.key === "End") updateSidebarWidth(MAX_SIDEBAR_WIDTH);
          else return;
          event.preventDefault();
        }}
      />
      <section id="workspace" aria-label="Workspace">
        {workspaceContent}
      </section>
    </main>
  );
}
