import { useEffect, useMemo, useState, type MouseEvent, type PointerEvent, type ReactElement, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Archive,
  ChevronRight,
  Copy,
  Folder,
  Plus,
  FolderOpen,
  Download,
  Minus,
  PanelLeft,
  PanelLeftDashed,
  Pin,
  PinOff,
  Square,
  Settings,
  Trash2,
  X,
  Upload,
  RefreshCw,
  FolderClosed,
  type LucideIcon,
} from "lucide-react";
import {
  chooseDirectory,
  chooseExportPath,
  chooseTarArchive,
  repositoryApi,
} from "./api";
import { activeRepository as findActiveRepository, repositoryByPath } from "./routing";
import {
  createState,
  createWorkspace,
  ensureWorkspace,
  loadState,
  reorderRepositories,
  reconcileSnapshotRoute,
  scheduleStateSave,
  setRepositoryPinned,
  upsertRepository,
  visiblePinnedRepositories,
  visibleUnpinnedRepositories,
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
type FormValidation<Field extends string> = { message: string; fields: Field[] };
type GlobalPage = "new" | "import" | null;
type WorkspaceModal = "add" | "export" | "restore" | "settings" | "deleteSnapshot" | null;
type SnapshotMap = Record<string, SnapshotInfo[] | undefined>;
type RepositorySectionKind = "pinned" | "repositories";
type RepositoryDragState = {
  path: string;
  name: string;
  section: RepositorySectionKind;
  startX: number;
  startY: number;
  offsetX: number;
  offsetY: number;
  x: number;
  y: number;
  isDragging: boolean;
  insertIndex: number;
};

const MIN_SIDEBAR_WIDTH = 220;
const MAX_SIDEBAR_WIDTH = 380;
const MIN_REFRESH_ANIMATION_MS = 450;
const REFRESH_FEEDBACK_DELAY_MS = 120;

function hasTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

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

function NoticeView({ notice }: { notice?: Notice }): ReactElement {
  return (
    <p className="page-notice" data-tone={notice?.tone ?? "info"} hidden={!notice?.message} aria-live="polite">
      {notice?.message ?? ""}
    </p>
  );
}

function InlineFormNotice({ notice }: { notice?: Notice }): ReactElement {
  return (
    <span className="form-inline-notice" data-tone={notice?.tone ?? "info"} hidden={!notice?.message} aria-live="polite">
      {notice?.message}
    </span>
  );
}

function inputStateClass(invalid: boolean): string | undefined {
  return invalid ? "is-invalid" : undefined;
}

function IconButton({
  icon: Icon,
  title,
  danger = false,
  disabled = false,
  className,
  onClick,
}: {
  icon: LucideIcon;
  title: string;
  danger?: boolean;
  disabled?: boolean;
  className?: string;
  onClick: (event: MouseEvent<HTMLButtonElement>) => void;
}): ReactElement {
  return (
    <button
      type="button"
      className={["icon-button", danger ? "is-danger" : "", className ?? ""].filter(Boolean).join(" ")}
      aria-label={title}
      title={title}
      disabled={disabled}
      onClick={onClick}
    >
      <Icon />
    </button>
  );
}

function AppTitleBar({
  sidebarCollapsed,
  onToggleSidebar,
}: {
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
}): ReactElement {
  const appWindow = useMemo(() => (hasTauriRuntime() ? getCurrentWindow() : undefined), []);
  const SidebarIcon = sidebarCollapsed ? PanelLeftDashed : PanelLeft;
  const [isMaximized, setIsMaximized] = useState(false);
  const MaximizeIcon = isMaximized ? Copy : Square;

  useEffect(() => {
    if (!appWindow) return undefined;
    let cancelled = false;
    const updateMaximized = (): void => {
      void appWindow.isMaximized().then((value) => {
        if (!cancelled) setIsMaximized(value);
      });
    };
    updateMaximized();
    let unlisten: (() => void) | undefined;
    void appWindow.onResized(() => updateMaximized()).then((handler) => {
      if (cancelled) handler();
      else unlisten = handler;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [appWindow]);

  return (
    <header id="app-titlebar">
      <div className="titlebar-leading">
        <IconButton
          className="titlebar-sidebar-toggle"
          icon={SidebarIcon}
          title={sidebarCollapsed ? "Show sidebar" : "Hide sidebar"}
          onClick={(event) => {
            event.stopPropagation();
            onToggleSidebar();
          }}
        />
      </div>
      <div
        className="titlebar-drag-region"
        onPointerDown={(event) => {
          if (event.button !== 0 || !appWindow) return;
          void appWindow.startDragging();
        }}
        onDoubleClick={() => {
          if (appWindow) void appWindow.toggleMaximize().then(() => appWindow.isMaximized()).then(setIsMaximized);
        }}
      />
      <div className="titlebar-window-controls">
        <IconButton
          className="titlebar-window-button"
          icon={Minus}
          title="Minimize"
          onClick={() => {
            if (appWindow) void appWindow.minimize();
          }}
        />
        <IconButton
          className={["titlebar-window-button titlebar-maximize-button", isMaximized ? "titlebar-restore-button" : ""].filter(Boolean).join(" ")}
          icon={MaximizeIcon}
          title="Maximize or restore"
          onClick={() => {
            if (appWindow) void appWindow.toggleMaximize().then(() => appWindow.isMaximized()).then(setIsMaximized);
          }}
        />
        <IconButton
          className="titlebar-window-button titlebar-close-button"
          icon={X}
          title="Close"
          onClick={() => {
            if (appWindow) void appWindow.close();
          }}
        />
      </div>
    </header>
  );
}

function WorkspaceModalView({
  title,
  onClose,
  children,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
}): ReactElement {
  return (
    <div className="modal-overlay" role="presentation" onMouseDown={onClose}>
      <section
        className="modal-window shadow"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onMouseDown={(event) => event.stopPropagation()}
      >
        {children}
      </section>
    </div>
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
  onToggleSection,
  onReorder,
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
  onToggleSection: (section: RepositorySectionKind) => void;
  onReorder: (section: RepositorySectionKind, orderedPaths: string[]) => void;
}): ReactElement {
  const pinnedRepositories = visiblePinnedRepositories(state);
  const repositories = visibleUnpinnedRepositories(state);
  return (
    <aside id="repository-sidebar" aria-label="Repositories">
      <header className="sidebar-header">
        <h1>BackupTool</h1>
      </header>
      <div className="sidebar-actions">
        <button className="sidebar-action" type="button" onClick={onNew} title="New repository">
          <Plus size={14} />
          <span>New</span>
        </button>
        <button className="sidebar-action" type="button" onClick={onOpen} title="Open repository">
          <FolderOpen size={14} />
          <span>Open</span>
        </button>
        <button className="sidebar-action" type="button" onClick={onImport} title="Import repository">
          <Download size={14} />
          <span>Import</span>
        </button>
      </div>
      <RepositorySection
        id="pinned-repository-list"
        title="Pinned"
        section="pinned"
        repositories={pinnedRepositories}
        expanded={state.sidebarSections.pinnedExpanded}
        unavailable={unavailable}
        globalPageActive={globalPageActive}
        activeRepositoryPath={state.activeRepositoryPath}
        onActivate={onActivate}
        onTogglePin={onTogglePin}
        onArchive={onArchive}
        onToggleSection={onToggleSection}
        onReorder={onReorder}
      />
      <RepositorySection
        id="repository-list"
        title="Repositories"
        section="repositories"
        repositories={repositories}
        expanded={state.sidebarSections.repositoriesExpanded}
        unavailable={unavailable}
        globalPageActive={globalPageActive}
        activeRepositoryPath={state.activeRepositoryPath}
        onActivate={onActivate}
        onTogglePin={onTogglePin}
        onArchive={onArchive}
        onToggleSection={onToggleSection}
        onReorder={onReorder}
      />
    </aside>
  );
}

function RepositorySection({
  id,
  title,
  section,
  repositories,
  expanded,
  unavailable,
  globalPageActive,
  activeRepositoryPath,
  onActivate,
  onTogglePin,
  onArchive,
  onToggleSection,
  onReorder,
}: {
  id: string;
  title: string;
  section: RepositorySectionKind;
  repositories: RepositoryRecord[];
  expanded: boolean;
  unavailable: ReadonlySet<string>;
  globalPageActive: boolean;
  activeRepositoryPath?: string;
  onActivate: (path: string) => void;
  onTogglePin: (path: string) => void;
  onArchive: (path: string) => void;
  onToggleSection: (section: RepositorySectionKind) => void;
  onReorder: (section: RepositorySectionKind, orderedPaths: string[]) => void;
}): ReactElement | null {
  const [dragState, setDragState] = useState<RepositoryDragState | undefined>();
  const isPinnedSection = section === "pinned";
  const draggedPath = dragState?.path;

  useEffect(() => {
    if (!dragState) return undefined;

    function findInsertIndex(clientY: number): number {
      const rows = Array.from(
        document.querySelectorAll<HTMLElement>(`.repository-row[data-repository-section="${section}"]`),
      );
      const nextIndex = rows.findIndex((row) => {
        const rect = row.getBoundingClientRect();
        return clientY < rect.top + rect.height / 2;
      });
      return nextIndex === -1 ? rows.length : nextIndex;
    }

    function handlePointerMove(event: globalThis.PointerEvent): void {
      const x = event.clientX;
      const y = event.clientY;
      setDragState((current) => {
        if (!current) return current;
        const distance = Math.hypot(x - current.startX, y - current.startY);
        const isDragging = current.isDragging || distance >= 4;
        return {
          ...current,
          x,
          y,
          isDragging,
          insertIndex: isDragging ? findInsertIndex(y) : current.insertIndex,
        };
      });
    }

    function handlePointerUp(): void {
      setDragState((current) => {
        if (!current) return current;
        if (!current.isDragging) {
          onActivate(current.path);
          return undefined;
        }
        const orderedPaths = repositories.map((repository) => repository.path);
        const fromIndex = orderedPaths.indexOf(current.path);
        if (fromIndex >= 0) {
          const [dragged] = orderedPaths.splice(fromIndex, 1);
          const adjustedInsertIndex =
            fromIndex < current.insertIndex ? current.insertIndex - 1 : current.insertIndex;
          orderedPaths.splice(Math.max(0, Math.min(adjustedInsertIndex, orderedPaths.length)), 0, dragged);
          onReorder(section, orderedPaths);
        }
        return undefined;
      });
    }

    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp, { once: true });
    window.addEventListener("pointercancel", handlePointerUp, { once: true });
    return () => {
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerUp);
      window.removeEventListener("pointercancel", handlePointerUp);
    };
  }, [dragState, onActivate, onReorder, repositories, section]);

  if (repositories.length === 0) return null;

  function startRepositoryDrag(event: PointerEvent<HTMLDivElement>, repository: RepositoryRecord): void {
    if (event.button !== 0 || (event.target as HTMLElement).closest("button")) return;
    event.preventDefault();
    const rect = event.currentTarget.getBoundingClientRect();
    const initialIndex = repositories.findIndex((item) => item.path === repository.path);
    setDragState({
      path: repository.path,
      name: repository.name,
      section,
      startX: event.clientX,
      startY: event.clientY,
      offsetX: event.clientX - rect.left,
      offsetY: event.clientY - rect.top,
      x: event.clientX,
      y: event.clientY,
      isDragging: false,
      insertIndex: initialIndex < 0 ? 0 : initialIndex,
    });
  }

  return (
    <section className="repository-section" aria-labelledby={`${id}-title`}>
      <button
        id={`${id}-title`}
        type="button"
        className="sidebar-section-title"
        aria-expanded={expanded}
        aria-controls={id}
        onClick={() => onToggleSection(section)}
      >
        <span>{title}</span>
        <ChevronRight className="section-chevron" data-expanded={expanded ? "true" : "false"} size={13} />
      </button>
      {expanded ? (
        <div className="repository-list" id={id}>
          {repositories.map((repository, index) => {
            const showInsertBefore =
              dragState?.isDragging &&
              dragState.insertIndex === index &&
              dragState.path !== repository.path &&
              repositories[index - 1]?.path !== dragState.path;
            const showInsertAfter =
              dragState?.isDragging &&
              index === repositories.length - 1 &&
              dragState.insertIndex === repositories.length &&
              dragState.path !== repository.path;
            return (
            <div
              className={[
                "repository-row",
                !globalPageActive && repository.path === activeRepositoryPath ? "is-active" : "",
                unavailable.has(repository.path) ? "is-unavailable" : "",
                draggedPath === repository.path ? "is-dragging" : "",
                showInsertBefore ? "is-insert-before" : "",
                showInsertAfter ? "is-insert-after" : "",
              ]
                .filter(Boolean)
                .join(" ")}
              key={repository.path}
              tabIndex={0}
              title={repository.path}
              data-repository-section={section}
              data-repository-path={repository.path}
              onPointerDown={(event) => startRepositoryDrag(event, repository)}
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
                <IconButton
                  className="row-action"
                  icon={isPinnedSection ? PinOff : Pin}
                  title={isPinnedSection ? "Unpin repository" : "Pin repository"}
                  onClick={(event) => {
                    event.stopPropagation();
                    onTogglePin(repository.path);
                  }}
                />
                <IconButton
                  className="row-action"
                  icon={Archive}
                  title="Archive repository"
                  onClick={(event) => {
                    event.stopPropagation();
                    onArchive(repository.path);
                  }}
                />
              </div>
            </div>
            );
          })}
          {dragState?.isDragging ? <RepositoryDragPreview dragState={dragState} /> : null}
        </div>
      ) : null}
    </section>
  );
}

function RepositoryDragPreview({ dragState }: { dragState: RepositoryDragState }): ReactElement {
  return (
    <div
      className="repository-drag-preview"
      style={{
        transform: `translate(${dragState.x - dragState.offsetX}px, ${dragState.y - dragState.offsetY}px)`,
      }}
    >
      <Folder size={14} />
      <span>{dragState.name}</span>
    </div>
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

function CenteredActionPanel({
  icon: Icon,
  title,
  titleTooltip,
  className,
  actions,
  children,
}: {
  icon: LucideIcon;
  title: string;
  titleTooltip?: string;
  className?: string;
  actions?: ReactNode;
  children: ReactNode;
}): ReactElement {
  return (
    <div className={["centered-action-panel", className ?? ""].filter(Boolean).join(" ")}>
      <div className="action-panel-heading">
        <div className="action-panel-title">
          <Icon size={22} />
          <h1 title={titleTooltip}>{title}</h1>
        </div>
        {actions ? <div className="action-panel-actions">{actions}</div> : null}
      </div>
      {children}
    </div>
  );
}

function NewRepositoryPage({
  notice,
  busy,
  onBrowse,
  onSubmit,
}: {
  notice?: Notice;
  busy: boolean;
  onBrowse: () => Promise<string | undefined>;
  onSubmit: (parentPath: string, name: string, encryptionAlgorithm: "none" | "aes-256-gcm", encryptionPassword: string) => Promise<boolean>;
}): ReactElement {
  const [parentPath, setParentPath] = useState("");
  const [name, setName] = useState("");
  const [encryptionAlgorithm, setEncryptionAlgorithm] = useState<"none" | "aes-256-gcm">("none");
  const [encryptionPassword, setEncryptionPassword] = useState("");
  const [invalidFields, setInvalidFields] = useState<Set<"parentPath" | "name" | "encryptionPassword">>(() => new Set());

  function validate(): FormValidation<"parentPath" | "name" | "encryptionPassword"> | undefined {
    const fields: Array<"parentPath" | "name" | "encryptionPassword"> = [];
    if (!parentPath.trim()) fields.push("parentPath");
    if (!name.trim()) fields.push("name");
    if (encryptionAlgorithm === "aes-256-gcm" && !encryptionPassword) fields.push("encryptionPassword");
    return fields.length > 0 ? { message: "Fill in the highlighted fields.", fields } : undefined;
  }

  function clearInvalid(field: "parentPath" | "name" | "encryptionPassword"): void {
    setInvalidFields((current) => {
      if (!current.has(field)) return current;
      const next = new Set(current);
      next.delete(field);
      return next;
    });
  }

  return (
    <CenteredActionPanel icon={Plus} title="New Repository">
      <form
        className="form-panel bordered shadow"
        onSubmit={async (event) => {
          event.preventDefault();
          const validation = validate();
          if (validation) {
            setInvalidFields(new Set(validation.fields));
            return;
          }
          const succeeded = await onSubmit(parentPath, name, encryptionAlgorithm, encryptionPassword);
          if (succeeded) setInvalidFields(new Set());
        }}
      >
        <label>
          Parent directory
          <span className="path-control">
            <input
              className={inputStateClass(invalidFields.has("parentPath"))}
              value={parentPath}
              onChange={(event) => {
                setParentPath(event.target.value);
                clearInvalid("parentPath");
              }}
              disabled={busy}
              autoComplete="off"
            />
            <button
              type="button"
              className="secondary-button bordered-button icon-button-text"
              disabled={busy}
              onClick={async () => {
                const selected = await onBrowse();
                if (selected) {
                  setParentPath(selected);
                  clearInvalid("parentPath");
                }
              }}
            >
              <FolderOpen size={14} />
              <span>Browse</span>
            </button>
          </span>
        </label>
        <label>
          Repository name
          <input
            className={inputStateClass(invalidFields.has("name"))}
            value={name}
            onChange={(event) => {
              setName(event.target.value);
              clearInvalid("name");
            }}
            disabled={busy}
            autoComplete="off"
            maxLength={120}
          />
        </label>
        <label>
          Encryption algorithm
          <select
            value={encryptionAlgorithm}
            disabled={busy}
            onChange={(event) => {
              const value = event.target.value === "aes-256-gcm" ? "aes-256-gcm" : "none";
              setEncryptionAlgorithm(value);
              if (value === "none") {
                setEncryptionPassword("");
                clearInvalid("encryptionPassword");
              }
            }}
          >
            <option value="none">None</option>
            <option value="aes-256-gcm">AES-256-GCM</option>
          </select>
        </label>
        {encryptionAlgorithm !== "none" ? (
          <label>
            Encryption password
            <input
              className={inputStateClass(invalidFields.has("encryptionPassword"))}
              value={encryptionPassword}
              onChange={(event) => {
                setEncryptionPassword(event.target.value);
                clearInvalid("encryptionPassword");
              }}
              disabled={busy}
              type="password"
              autoComplete="new-password"
            />
          </label>
        ) : null}
        <div className="form-actions">
          <InlineFormNotice notice={invalidFields.size > 0 ? { tone: "error", message: "Fill in the highlighted fields." } : notice} />
          <button type="submit" className="primary-button" disabled={busy}>
            Create Repository
          </button>
        </div>
      </form>
    </CenteredActionPanel>
  );
}

function ImportRepositoryPage({
  notice,
  busy,
  onBrowseArchive,
  onBrowseDestination,
  onSubmit,
}: {
  notice?: Notice;
  busy: boolean;
  onBrowseArchive: () => Promise<string | undefined>;
  onBrowseDestination: () => Promise<string | undefined>;
  onSubmit: (archivePath: string, destination: string) => Promise<boolean>;
}): ReactElement {
  const [archivePath, setArchivePath] = useState("");
  const [destination, setDestination] = useState("");
  const [invalidFields, setInvalidFields] = useState<Set<"archivePath" | "destination">>(() => new Set());

  function validate(): FormValidation<"archivePath" | "destination"> | undefined {
    const fields: Array<"archivePath" | "destination"> = [];
    if (!archivePath.trim()) fields.push("archivePath");
    if (!destination.trim()) fields.push("destination");
    return fields.length > 0 ? { message: "Fill in the highlighted fields.", fields } : undefined;
  }

  function clearInvalid(field: "archivePath" | "destination"): void {
    setInvalidFields((current) => {
      if (!current.has(field)) return current;
      const next = new Set(current);
      next.delete(field);
      return next;
    });
  }

  return (
    <CenteredActionPanel icon={Download} title="Import Repository">
      <form
        className="form-panel bordered shadow"
        onSubmit={async (event) => {
          event.preventDefault();
          const validation = validate();
          if (validation) {
            setInvalidFields(new Set(validation.fields));
            return;
          }
          const succeeded = await onSubmit(archivePath, destination);
          if (succeeded) setInvalidFields(new Set());
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
            <input
              className={inputStateClass(invalidFields.has("archivePath"))}
              value={archivePath}
              onChange={(event) => {
                setArchivePath(event.target.value);
                clearInvalid("archivePath");
              }}
              disabled={busy}
              autoComplete="off"
            />
            <button
              type="button"
              className="secondary-button bordered-button icon-button-text"
              disabled={busy}
              onClick={async () => {
                const selected = await onBrowseArchive();
                if (selected) {
                  setArchivePath(selected);
                  clearInvalid("archivePath");
                }
              }}
            >
              <FolderOpen size={14} />
              <span>Browse</span>
            </button>
          </span>
        </label>
        <label>
          Destination directory
          <span className="path-control">
            <input
              className={inputStateClass(invalidFields.has("destination"))}
              value={destination}
              onChange={(event) => {
                setDestination(event.target.value);
                clearInvalid("destination");
              }}
              disabled={busy}
              autoComplete="off"
            />
            <button
              type="button"
              className="secondary-button bordered-button icon-button-text"
              disabled={busy}
              onClick={async () => {
                const selected = await onBrowseDestination();
                if (selected) {
                  setDestination(selected);
                  clearInvalid("destination");
                }
              }}
            >
              <FolderOpen size={14} />
              <span>Browse</span>
            </button>
          </span>
        </label>
        <div className="form-actions">
          <InlineFormNotice notice={invalidFields.size > 0 ? { tone: "error", message: "Fill in the highlighted fields." } : notice} />
          <button type="submit" className="primary-button" disabled={busy}>
            Import Repository
          </button>
        </div>
      </form>
    </CenteredActionPanel>
  );
}

function SnapshotList({
  snapshots,
  busySnapshotId,
  onRestore,
  onDelete,
}: {
  snapshots: SnapshotInfo[] | undefined;
  busySnapshotId?: string;
  onRestore: (snapshotId: string) => void;
  onDelete: (snapshot: SnapshotInfo) => void;
}): ReactElement {
  return (
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
          <IconButton
            className="snapshot-delete"
            icon={X}
            title="Delete snapshot"
            danger
            disabled={busySnapshotId === snapshot.id}
            onClick={(event) => {
              event.stopPropagation();
              onDelete(snapshot);
            }}
          />
        </article>
      ))}
    </div>
  );
}

function ViewRepositoryPage({
  snapshots,
  refreshFeedback,
  isRefreshing,
  busySnapshotId,
  onAdd,
  onExport,
  onRefresh,
  onRestore,
  onDelete,
}: {
  snapshots: SnapshotInfo[] | undefined;
  refreshFeedback?: { text: string; id: number };
  isRefreshing: boolean;
  busySnapshotId?: string;
  onAdd: () => void;
  onExport: () => void;
  onRefresh: () => void;
  onRestore: (snapshotId: string) => void;
  onDelete: (snapshot: SnapshotInfo) => void;
}): ReactElement {
  return (
    <form
      className="form-panel bordered shadow repository-view-panel"
      onSubmit={(event) => {
        event.preventDefault();
      }}
    >
      <div className="snapshot-toolbar">
        <div className="snapshot-toolbar-actions">
          <button type="button" className="primary-button icon-button-text" onClick={onAdd}>
            <Plus size={14} />
            <span>Add Snapshot</span>
          </button>
          <button type="button" className="secondary-button icon-button-text" onClick={onExport}>
            <Upload size={14} />
            <span>Export Repository</span>
          </button>
        </div>
        <div className="snapshot-refresh-group">
          {refreshFeedback ? (
            <span className="refresh-feedback" key={refreshFeedback.id}>
              {refreshFeedback.text}
            </span>
          ) : null}
          <button type="button" className="refresh-button icon-button-text" onClick={onRefresh} aria-busy={isRefreshing}>
            <RefreshCw className="refresh-icon" size={14} data-refreshing={isRefreshing ? "true" : "false"} />
            <span>Refresh</span>
          </button>
        </div>
      </div>
      <SnapshotList
        snapshots={snapshots}
        busySnapshotId={busySnapshotId}
        onRestore={onRestore}
        onDelete={onDelete}
      />
    </form>
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
          <IconButton icon={X} title="Remove source" onClick={() => onRemove(index)} />
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
  onBrowseSource,
  onChangeDraft,
  onChangeFilter,
  onRemoveSource,
  onSubmit,
}: {
  repository: RepositoryRecord;
  draft: RepositoryWorkspace;
  notice?: Notice;
  busy: boolean;
  onBrowseSource: () => void;
  onChangeDraft: <K extends keyof RepositoryWorkspace>(field: K, value: RepositoryWorkspace[K]) => void;
  onChangeFilter: <K extends keyof BackupFilterDraft>(field: K, value: BackupFilterDraft[K]) => void;
  onRemoveSource: (index: number) => void;
  onSubmit: () => Promise<boolean>;
}): ReactElement {
  type AddField = "sources" | "pathRegex" | "minSize" | "maxSize" | "modifiedAfter" | "modifiedBefore";
  const [invalidFields, setInvalidFields] = useState<Set<AddField>>(() => new Set());

  function validate(): FormValidation<AddField> | undefined {
    const fields: AddField[] = [];
    const minimum = draft.filter.minSize ? Number(draft.filter.minSize) : undefined;
    const maximum = draft.filter.maxSize ? Number(draft.filter.maxSize) : undefined;
    if (draft.sourcePaths.length === 0) fields.push("sources");
    if (draft.filter.pathRegex.trim()) {
      try {
        new RegExp(draft.filter.pathRegex);
      } catch {
        fields.push("pathRegex");
      }
    }
    if (minimum !== undefined && maximum !== undefined && minimum > maximum) fields.push("minSize", "maxSize");
    if (
      draft.filter.modifiedAfter &&
      draft.filter.modifiedBefore &&
      new Date(draft.filter.modifiedAfter) > new Date(draft.filter.modifiedBefore)
    ) {
      fields.push("modifiedAfter", "modifiedBefore");
    }
    return fields.length > 0 ? { message: "Fix the highlighted fields.", fields } : undefined;
  }

  function clearInvalid(field: AddField): void {
    setInvalidFields((current) => {
      if (!current.has(field)) return current;
      const next = new Set(current);
      next.delete(field);
      return next;
    });
  }

  function clearInvalidFields(fields: AddField[]): void {
    setInvalidFields((current) => {
      if (!fields.some((field) => current.has(field))) return current;
      const next = new Set(current);
      for (const field of fields) next.delete(field);
      return next;
    });
  }

  useEffect(() => {
    if (draft.sourcePaths.length > 0) clearInvalid("sources");
  }, [draft.sourcePaths.length]);

  return (
    <form
      className="form-panel"
      aria-label={`Add snapshot to ${repository.name}`}
      onSubmit={async (event) => {
        event.preventDefault();
        const validation = validate();
        if (validation) {
          setInvalidFields(new Set(validation.fields));
          return;
        }
        const succeeded = await onSubmit();
        if (succeeded) setInvalidFields(new Set());
      }}
    >
      <div className="field-group">
        <div className="group-heading">
          <div>
            <h2>Source directories</h2>
            <p>Add one or more directories to this snapshot.</p>
          </div>
          <button type="button" className="secondary-button bordered-button icon-button-text" onClick={onBrowseSource} disabled={busy}>
            <Plus size={14} />
            <span>Add</span>
          </button>
        </div>
        <div className={invalidFields.has("sources") ? "is-invalid-container" : undefined}>
          <SourceList sourcePaths={draft.sourcePaths} onRemove={(index) => {
            onRemoveSource(index);
            clearInvalid("sources");
          }} />
        </div>
      </div>
      <div className="form-grid two-columns">
        <label className="full-row">
          Snapshot title
          <input value={draft.snapshotTitle} disabled={busy} onChange={(event) => onChangeDraft("snapshotTitle", event.target.value)} type="text" maxLength={120} autoComplete="off" />
        </label>
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
        {repository.encryptionAlgorithm !== "none" ? (
          <label>
            Encrypt this snapshot
            <select
              value={draft.encryptSnapshot ? "true" : "false"}
              disabled={busy}
              onChange={(event) => onChangeDraft("encryptSnapshot", event.target.value === "true")}
            >
              <option value="false">No</option>
              <option value="true">Yes</option>
            </select>
          </label>
        ) : null}
      </div>
      <details className="advanced-panel" open={draft.filtersOpen} onToggle={(event) => onChangeDraft("filtersOpen", event.currentTarget.open)}>
        <summary>Advanced filters</summary>
        <div className="form-grid three-columns">
          <label className="full-row">Path regex<input className={inputStateClass(invalidFields.has("pathRegex"))} value={draft.filter.pathRegex} disabled={busy} onChange={(event) => {
            onChangeFilter("pathRegex", event.target.value);
            clearInvalid("pathRegex");
          }} autoComplete="off" placeholder=".*\\.txt$" /></label>
          <label>Minimum size<input className={inputStateClass(invalidFields.has("minSize"))} value={draft.filter.minSize} disabled={busy} onChange={(event) => {
            onChangeFilter("minSize", event.target.value);
            clearInvalidFields(["minSize", "maxSize"]);
          }} type="number" min="0" step="1" /></label>
          <label>Maximum size<input className={inputStateClass(invalidFields.has("maxSize"))} value={draft.filter.maxSize} disabled={busy} onChange={(event) => {
            onChangeFilter("maxSize", event.target.value);
            clearInvalidFields(["minSize", "maxSize"]);
          }} type="number" min="0" step="1" /></label>
          <label>Modified after<input className={inputStateClass(invalidFields.has("modifiedAfter"))} value={draft.filter.modifiedAfter} disabled={busy} onChange={(event) => {
            onChangeFilter("modifiedAfter", event.target.value);
            clearInvalidFields(["modifiedAfter", "modifiedBefore"]);
          }} type="datetime-local" /></label>
          <label>Modified before<input className={inputStateClass(invalidFields.has("modifiedBefore"))} value={draft.filter.modifiedBefore} disabled={busy} onChange={(event) => {
            onChangeFilter("modifiedBefore", event.target.value);
            clearInvalidFields(["modifiedAfter", "modifiedBefore"]);
          }} type="datetime-local" /></label>
        </div>
      </details>
      <div className="form-actions">
        <InlineFormNotice notice={invalidFields.size > 0 ? { tone: "error", message: "Fix the highlighted fields." } : notice} />
        <button type="submit" className="primary-button" disabled={busy}>
          Add Snapshot
        </button>
      </div>
    </form>
  );
}

function ExportRepositoryPage({
  repository,
  draft,
  notice,
  busy,
  onChangeExportPath,
  onBrowse,
  onSubmit,
}: {
  repository: RepositoryRecord;
  draft: RepositoryWorkspace;
  notice?: Notice;
  busy: boolean;
  onChangeExportPath: (value: string) => void;
  onBrowse: () => void;
  onSubmit: () => Promise<boolean>;
}): ReactElement {
  const [invalidFields, setInvalidFields] = useState<Set<"exportPath">>(() => new Set());

  function clearInvalid(): void {
    setInvalidFields((current) => {
      if (!current.has("exportPath")) return current;
      return new Set();
    });
  }

  useEffect(() => {
    if (draft.exportPath.trim()) clearInvalid();
  }, [draft.exportPath]);

  return (
    <form
      className="form-panel"
      aria-label={`Export repository ${repository.name}`}
      onSubmit={async (event) => {
        event.preventDefault();
        if (!draft.exportPath.trim()) {
          setInvalidFields(new Set(["exportPath"]));
          return;
        }
        const succeeded = await onSubmit();
        if (succeeded) setInvalidFields(new Set());
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
          <input
            className={inputStateClass(invalidFields.has("exportPath"))}
            value={draft.exportPath}
            disabled={busy}
            onChange={(event) => {
              onChangeExportPath(event.target.value);
              clearInvalid();
            }}
            autoComplete="off"
          />
          <button type="button" className="secondary-button bordered-button icon-button-text" disabled={busy} onClick={onBrowse}>
            <FolderOpen size={14} />
            <span>Browse</span>
          </button>
        </span>
      </label>
      <div className="form-actions">
        <InlineFormNotice notice={invalidFields.size > 0 ? { tone: "error", message: "Choose an export file." } : notice} />
        <button type="submit" className="primary-button" disabled={busy}>
          Export Repository
        </button>
      </div>
    </form>
  );
}

function RepositorySettingsPage({
  repository,
  notice,
  busy,
  repositoryUnlocked,
  onRename,
  onUnlock,
  onChangePassword,
  onDelete,
}: {
  repository: RepositoryRecord;
  notice?: Notice;
  busy: boolean;
  repositoryUnlocked: boolean;
  onRename: (name: string) => Promise<boolean>;
  onUnlock: (password: string) => Promise<boolean>;
  onChangePassword: (oldPassword: string, newPassword: string) => Promise<boolean>;
  onDelete: (password?: string) => Promise<boolean>;
}): ReactElement {
  const [name, setName] = useState(repository.name);
  const [unlockPassword, setUnlockPassword] = useState("");
  const [oldPassword, setOldPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [deletePassword, setDeletePassword] = useState("");
  const [invalidName, setInvalidName] = useState(false);
  const [invalidPassword, setInvalidPassword] = useState(false);
  const [invalidPasswordChangeFields, setInvalidPasswordChangeFields] = useState<Set<"oldPassword" | "newPassword" | "confirmPassword">>(() => new Set());
  const [invalidDeletePassword, setInvalidDeletePassword] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  useEffect(() => {
    setName(repository.name);
    setUnlockPassword("");
    setOldPassword("");
    setNewPassword("");
    setConfirmPassword("");
    setDeletePassword("");
    setInvalidName(false);
    setInvalidPassword(false);
    setInvalidPasswordChangeFields(new Set());
    setInvalidDeletePassword(false);
    setConfirmDelete(false);
  }, [repository.path, repository.name]);

  async function saveName(): Promise<void> {
    const trimmed = name.trim();
    if (!trimmed) {
      setInvalidName(true);
      return;
    }
    if (trimmed === repository.name) return;
    const succeeded = await onRename(trimmed);
    if (!succeeded) setName(repository.name);
  }

  async function unlockRepository(): Promise<void> {
    if (repository.encryptionAlgorithm === "none" || repositoryUnlocked) return;
    if (!unlockPassword) {
      setInvalidPassword(true);
      return;
    }
    const succeeded = await onUnlock(unlockPassword);
    if (succeeded) {
      setUnlockPassword("");
      setInvalidPassword(false);
    } else {
      setInvalidPassword(true);
    }
  }

  function clearPasswordChangeInvalid(field: "oldPassword" | "newPassword" | "confirmPassword"): void {
    setInvalidPasswordChangeFields((current) => {
      if (!current.has(field)) return current;
      const next = new Set(current);
      next.delete(field);
      return next;
    });
  }

  async function changePassword(): Promise<void> {
    const fields: Array<"oldPassword" | "newPassword" | "confirmPassword"> = [];
    if (!oldPassword) fields.push("oldPassword");
    if (!newPassword) fields.push("newPassword");
    if (newPassword !== confirmPassword) fields.push("confirmPassword");
    if (fields.length > 0) {
      setInvalidPasswordChangeFields(new Set(fields));
      return;
    }
    const succeeded = await onChangePassword(oldPassword, newPassword);
    if (succeeded) {
      setOldPassword("");
      setNewPassword("");
      setConfirmPassword("");
      setInvalidPasswordChangeFields(new Set());
    } else {
      setInvalidPasswordChangeFields(new Set(["oldPassword"]));
    }
  }

  return (
    <form
      className="form-panel"
      aria-label={`Repository settings for ${repository.name}`}
      onSubmit={async (event) => {
        event.preventDefault();
        await saveName();
      }}
    >
      <label>
        Repository name
        <input
          className={inputStateClass(invalidName)}
          value={name}
          disabled={busy}
          onChange={(event) => {
            setName(event.target.value);
            setInvalidName(false);
          }}
          onBlur={() => void saveName()}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              void saveName();
            }
          }}
          autoComplete="off"
          maxLength={120}
        />
      </label>
      {repository.encryptionAlgorithm !== "none" ? (
        <div className="unlock-zone">
          <label>
            Encryption password
            <input
              className={inputStateClass(invalidPassword)}
              value={repositoryUnlocked ? "Repository unlocked" : unlockPassword}
              disabled={busy || repositoryUnlocked}
              onChange={(event) => {
                setUnlockPassword(event.target.value);
                setInvalidPassword(false);
              }}
              type={repositoryUnlocked ? "text" : "password"}
              autoComplete="current-password"
            />
          </label>
          <div className="form-actions">
            <InlineFormNotice
              notice={
                invalidPassword
                  ? { tone: "error", message: "Password must not be empty." }
                  : repositoryUnlocked
                    ? { tone: "success", message: "Repository encryption is unlocked for this session." }
                    : undefined
              }
            />
            <button type="button" className="primary-button" disabled={busy || repositoryUnlocked} onClick={() => void unlockRepository()}>
              Unlock
            </button>
          </div>
        </div>
      ) : null}
      {repository.encryptionAlgorithm !== "none" ? (
        <div className="password-zone">
          <h2>Change password</h2>
          <label>
            Old password
            <input
              className={inputStateClass(invalidPasswordChangeFields.has("oldPassword"))}
              value={oldPassword}
              disabled={busy}
              onChange={(event) => {
                setOldPassword(event.target.value);
                clearPasswordChangeInvalid("oldPassword");
              }}
              type="password"
              autoComplete="current-password"
            />
          </label>
          <label>
            New password
            <input
              className={inputStateClass(invalidPasswordChangeFields.has("newPassword"))}
              value={newPassword}
              disabled={busy}
              onChange={(event) => {
                setNewPassword(event.target.value);
                clearPasswordChangeInvalid("newPassword");
              }}
              type="password"
              autoComplete="new-password"
            />
          </label>
          <label>
            Confirm new password
            <input
              className={inputStateClass(invalidPasswordChangeFields.has("confirmPassword"))}
              value={confirmPassword}
              disabled={busy}
              onChange={(event) => {
                setConfirmPassword(event.target.value);
                clearPasswordChangeInvalid("confirmPassword");
              }}
              type="password"
              autoComplete="new-password"
            />
          </label>
          <div className="form-actions">
            <InlineFormNotice
              notice={
                invalidPasswordChangeFields.size > 0
                  ? { tone: "error", message: "Fill in the highlighted password fields." }
                  : undefined
              }
            />
            <button type="button" className="primary-button" disabled={busy} onClick={() => void changePassword()}>
              Change Password
            </button>
          </div>
        </div>
      ) : null}
      <div className="danger-zone">
        <h2>Danger zone</h2>
        {!confirmDelete ? (
          <button type="button" className="danger-button icon-button-text" disabled={busy} onClick={() => setConfirmDelete(true)}>
            <Trash2 size={14} />
            <span>Delete Repository</span>
          </button>
        ) : (
          <div className="confirm-panel">
            <p>Delete repository "{repository.name}" from disk? This cannot be undone.</p>
            {repository.encryptionAlgorithm !== "none" ? (
              <label>
                Encryption password
                <input
                  className={inputStateClass(invalidDeletePassword)}
                  value={deletePassword}
                  disabled={busy}
                  onChange={(event) => {
                    setDeletePassword(event.target.value);
                    setInvalidDeletePassword(false);
                  }}
                  type="password"
                  autoComplete="current-password"
                />
              </label>
            ) : null}
            <div className="form-actions">
              <button type="button" className="secondary-button" disabled={busy} onClick={() => setConfirmDelete(false)}>
                Cancel
              </button>
              <button
                type="button"
                className="danger-button icon-button-text"
                disabled={busy}
                onClick={async () => {
                  if (repository.encryptionAlgorithm !== "none" && !deletePassword) {
                    setInvalidDeletePassword(true);
                    return;
                  }
                  const succeeded = await onDelete(repository.encryptionAlgorithm !== "none" ? deletePassword : undefined);
                  if (!succeeded) setInvalidDeletePassword(repository.encryptionAlgorithm !== "none");
                }}
              >
                <Trash2 size={14} />
                <span>Delete Repository</span>
              </button>
            </div>
          </div>
        )}
      </div>
      <div className="form-actions">
        <InlineFormNotice notice={invalidName ? { tone: "error", message: "Repository name must not be empty." } : notice} />
      </div>
    </form>
  );
}

function RestoreSnapshotPage({
  repository,
  snapshot,
  draft,
  notice,
  busy,
  decryptionPassword,
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
  onChangeDraft: <K extends keyof RepositoryWorkspace>(field: K, value: RepositoryWorkspace[K]) => void;
  onPasswordChange: (value: string) => void;
  onBrowse: () => void;
  onSubmit: () => Promise<boolean>;
}): ReactElement {
  const [invalidPassword, setInvalidPassword] = useState(false);

  return (
    <form
      className="form-panel"
      aria-label={`Restore snapshot ${snapshot.title?.trim() || "Untitled"} from ${repository.name}`}
      onSubmit={(event) => {
        event.preventDefault();
        if (snapshot.hasEncryptedObjects && !decryptionPassword) {
          setInvalidPassword(true);
          return;
        }
        void onSubmit().then((succeeded) => {
          if (!succeeded && snapshot.hasEncryptedObjects) setInvalidPassword(true);
        });
      }}
    >
      <div className="snapshot-summary">
        <span>Snapshot</span><strong>{snapshot.title?.trim() || "Untitled"}</strong>
        <span>Created</span><strong>{formatSnapshotTime(snapshot)}</strong>
        <span>Snapshot ID</span><code>{snapshot.id}</code>
      </div>
      <label>
        Restore directory
        <span className="path-control">
          <input value={draft.restoreDestination} disabled={busy} onChange={(event) => onChangeDraft("restoreDestination", event.target.value)} autoComplete="off" />
          <button type="button" className="secondary-button bordered-button icon-button-text" disabled={busy} onClick={onBrowse}>
            <FolderOpen size={14} />
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
      {snapshot.hasEncryptedObjects ? (
        <label>
          Decryption password
          <input
            className={inputStateClass(invalidPassword)}
            value={decryptionPassword}
            disabled={busy}
            onChange={(event) => {
              onPasswordChange(event.target.value);
              setInvalidPassword(false);
            }}
            type="password"
            autoComplete="current-password"
          />
        </label>
      ) : null}
      <div className="form-actions">
        <InlineFormNotice notice={invalidPassword ? { tone: "error", message: "Password is required for this encrypted snapshot." } : undefined} />
        <button type="submit" className="primary-button" disabled={busy}>
          Restore Snapshot
        </button>
      </div>
      <NoticeView notice={notice} />
    </form>
  );
}

function DeleteSnapshotPage({
  snapshot,
  notice,
  busy,
  onCancel,
  onSubmit,
}: {
  snapshot: SnapshotInfo;
  notice?: Notice;
  busy: boolean;
  onCancel: () => void;
  onSubmit: (password?: string) => Promise<boolean>;
}): ReactElement {
  const [password, setPassword] = useState("");
  const [invalidPassword, setInvalidPassword] = useState(false);
  return (
    <form
      className="form-panel"
      aria-label={`Delete snapshot ${snapshot.title?.trim() || "Untitled"}`}
      onSubmit={async (event) => {
        event.preventDefault();
        if (snapshot.hasEncryptedObjects && !password) {
          setInvalidPassword(true);
          return;
        }
        const succeeded = await onSubmit(snapshot.hasEncryptedObjects ? password : undefined);
        if (!succeeded && snapshot.hasEncryptedObjects) setInvalidPassword(true);
      }}
    >
      <div className="snapshot-summary">
        <span>Snapshot</span><strong>{snapshot.title?.trim() || "Untitled"}</strong>
        <span>Created</span><strong>{formatSnapshotTime(snapshot)}</strong>
        <span>Snapshot ID</span><code>{snapshot.id}</code>
      </div>
      <p className="danger-copy">Delete this snapshot? Unreferenced objects will also be removed.</p>
      {snapshot.hasEncryptedObjects ? (
        <label>
          Encryption password
          <input
            className={inputStateClass(invalidPassword)}
            value={password}
            disabled={busy}
            onChange={(event) => {
              setPassword(event.target.value);
              setInvalidPassword(false);
            }}
            type="password"
            autoComplete="current-password"
          />
        </label>
      ) : null}
      <div className="form-actions">
        <InlineFormNotice
          notice={
            invalidPassword
              ? { tone: "error", message: "Correct password is required to delete this encrypted snapshot." }
              : notice
          }
        />
        <button type="button" className="secondary-button" disabled={busy} onClick={onCancel}>
          Cancel
        </button>
        <button type="submit" className="danger-button icon-button-text" disabled={busy}>
          <Trash2 size={14} />
          <span>Delete Snapshot</span>
        </button>
      </div>
    </form>
  );
}

export function App(): ReactElement {
  const [state, setState] = useState<AppState | null>(null);
  const [globalPage, setGlobalPage] = useState<GlobalPage>(null);
  const [workspaceModal, setWorkspaceModal] = useState<WorkspaceModal>(null);
  const [globalNotice, setGlobalNotice] = useState<Notice | undefined>();
  const [notices, setNotices] = useState<Record<string, Notice>>({});
  const [snapshots, setSnapshots] = useState<SnapshotMap>({});
  const [refreshFeedback, setRefreshFeedback] = useState<{ text: string; id: number } | undefined>();
  const [refreshRequestInFlight, setRefreshRequestInFlight] = useState(false);
  const [isRefreshingSnapshots, setIsRefreshingSnapshots] = useState(false);
  const [unavailable, setUnavailable] = useState<Set<string>>(() => new Set());
  const [isResizing, setIsResizing] = useState(false);
  const [busy, setBusy] = useState<string | undefined>();
  const [busySnapshotId, setBusySnapshotId] = useState<string | undefined>();
  const [repositoryPasswords, setRepositoryPasswords] = useState<Record<string, string>>({});
  const [decryptionPassword, setDecryptionPassword] = useState("");
  const [pendingDeleteSnapshot, setPendingDeleteSnapshot] = useState<SnapshotInfo | undefined>();

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
                repository.encryptionAlgorithm = info.encryptionAlgorithm;
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
    setDecryptionPassword("");
  }

function closeWorkspaceModal(): void {
    if (workspace?.route.kind === "restore") setRoute({ kind: "overview" });
    setWorkspaceModal(null);
    setPendingDeleteSnapshot(undefined);
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

  async function refreshSnapshots(path: string, announce = true): Promise<number | undefined> {
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
      return list.length;
    } catch (error) {
      setUnavailable((current) => new Set(current).add(path));
      setNotice(path, "overview", { tone: "error", message: String(error) });
      return undefined;
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

  async function activateRepository(path: string): Promise<void> {
    try {
      const info = await repositoryApi.open(path);
      let openedPath = info.path;
      updateState((draft) => {
        openedPath = upsertRepository(draft, info, { updateLastOpenedAt: false }).path;
      });
      setGlobalPage(null);
      setUnavailable((current) => {
        const next = new Set(current);
        next.delete(path);
        next.delete(openedPath);
        return next;
      });
      await refreshSnapshots(openedPath, false);
    } catch (error) {
      setUnavailable((current) => new Set(current).add(path));
      setNotice(path, "overview", { tone: "error", message: String(error) });
    }
  }

  function changeActiveRepository(path: string): void {
    void activateRepository(path);
  }

  function togglePin(path: string): void {
    updateState((draft) => {
      const repository = repositoryByPath(draft, path);
      if (repository) setRepositoryPinned(draft, path, !repository.pinned);
    });
  }

  function toggleSidebarSection(section: "pinned" | "repositories"): void {
    updateState((draft) => {
      if (section === "pinned") {
        draft.sidebarSections.pinnedExpanded = !draft.sidebarSections.pinnedExpanded;
      } else {
        draft.sidebarSections.repositoriesExpanded = !draft.sidebarSections.repositoriesExpanded;
      }
    });
  }

  function reorderRepositorySection(section: "pinned" | "repositories", orderedPaths: string[]): void {
    updateState((draft) => {
      reorderRepositories(draft, section === "pinned", orderedPaths);
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

  function toggleSidebarCollapsed(): void {
    updateState((draft) => {
      draft.sidebarCollapsed = !draft.sidebarCollapsed;
    });
    setIsResizing(false);
  }

  if (!state) {
    return (
      <main id="app-shell" className="is-sidebar-collapsed">
        <AppTitleBar sidebarCollapsed={false} onToggleSidebar={() => undefined} />
        <div id="app-content">
          <section id="workspace" className="shadow" aria-label="Workspace">
            <EmptyWorkspace />
          </section>
        </div>
      </main>
    );
  }

  function workspaceNotice(route: WorkspaceRoute["kind"]): Notice | undefined {
    return active ? notices[noticeKey(active.path, route)] : undefined;
  }

  const workspaceContent = (() => {
    if (globalPage === "new") {
      return (
        <NewRepositoryPage
          notice={globalNotice}
          busy={busy === "new"}
          onBrowse={() => chooseDirectory("Choose repository parent")}
          onSubmit={async (parentPath, name, encryptionAlgorithm, encryptionPassword) => {
            setBusy("new");
            setGlobalNotice({ tone: "info", message: "Creating repository..." });
            try {
              const info = await repositoryApi.create(parentPath, name, encryptionAlgorithm, encryptionPassword);
              let openedPath = info.path;
              updateState((draft) => {
                openedPath = upsertRepository(draft, info).path;
              });
              setGlobalPage(null);
              setGlobalNotice(undefined);
              if (encryptionAlgorithm === "aes-256-gcm") {
                setRepositoryPasswords((current) => ({ ...current, [openedPath]: encryptionPassword }));
              }
              setNotice(openedPath, "overview", { tone: "success", message: "Repository created." });
              await refreshSnapshots(openedPath, false);
              return true;
            } catch (error) {
              setGlobalNotice({ tone: "error", message: String(error) });
              return false;
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
              return true;
            } catch (error) {
              setGlobalNotice({ tone: "error", message: String(error) });
              return false;
            } finally {
              setBusy(undefined);
            }
          }}
        />
      );
    }
    if (!active || active.archived || !workspace) return <EmptyWorkspace />;

    return (
      <CenteredActionPanel
        icon={FolderClosed}
        title={active.name}
        titleTooltip={active.path}
        actions={
          <button
            type="button"
            className="refresh-button settings-button"
            aria-label="Repository settings"
            title="Repository settings"
            onClick={() => setWorkspaceModal("settings")}
          >
            <Settings size={14} />
          </button>
        }
      >
        <ViewRepositoryPage
          snapshots={snapshots[active.path]}
          refreshFeedback={refreshFeedback}
          isRefreshing={isRefreshingSnapshots}
          busySnapshotId={busySnapshotId}
          onAdd={() => setWorkspaceModal("add")}
          onExport={() => setWorkspaceModal("export")}
          onRefresh={async () => {
            if (refreshRequestInFlight) return;
            const before = snapshots[active.path]?.length ?? 0;
            const animationStartedAt = performance.now();
            setRefreshFeedback(undefined);
            setRefreshRequestInFlight(true);
            setIsRefreshingSnapshots(true);
            let nextFeedback: { text: string; id: number } | undefined;
            try {
              const after = await refreshSnapshots(active.path, false);
              if (after === undefined) return;
              const delta = after - before;
              nextFeedback = {
                text: delta >= 0 ? `+${delta}` : String(delta),
                id: Date.now(),
              };
            } finally {
              setRefreshRequestInFlight(false);
              const elapsed = performance.now() - animationStartedAt;
              const remaining = Math.max(0, MIN_REFRESH_ANIMATION_MS - elapsed);
              window.setTimeout(() => {
                setIsRefreshingSnapshots(false);
                if (nextFeedback) window.setTimeout(() => setRefreshFeedback(nextFeedback), REFRESH_FEEDBACK_DELAY_MS);
              }, remaining);
            }
          }}
          onRestore={(snapshotId) => {
            setRoute({ kind: "restore", snapshotId });
            setWorkspaceModal("restore");
          }}
          onDelete={async (snapshot) => {
            if (snapshot.hasEncryptedObjects) {
              setPendingDeleteSnapshot(snapshot);
              setWorkspaceModal("deleteSnapshot");
              return;
            }
            if (!window.confirm(`Delete snapshot "${snapshot.title?.trim() || "Untitled"}"? Unreferenced objects will also be removed.`)) return;
            setBusySnapshotId(snapshot.id);
            setNotice(active.path, "overview", { tone: "info", message: "Deleting snapshot..." });
            try {
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
      </CenteredActionPanel>
    );
  })();

  const modalContent =
    active && workspace && workspaceModal === "add" ? (
      <WorkspaceModalView title="Add Snapshot" onClose={closeWorkspaceModal}>
        <AddSnapshotPage
          repository={active}
          draft={workspace}
          notice={workspaceNotice("add")}
          busy={busy === "backup"}
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
          onSubmit={async () => {
            setBusy("backup");
            setNotice(active.path, "add", { tone: "info", message: "Adding snapshot..." });
            try {
              const result = await repositoryApi.backup({
                repositoryPath: active.path,
                sources: workspace.sourcePaths,
                filter: workspace.filter,
                compressionAlgorithm: workspace.compressionAlgorithm,
                encryptSnapshot: active.encryptionAlgorithm !== "none" && workspace.encryptSnapshot,
                encryptionPassword: repositoryPasswords[active.path] ?? "",
                snapshotTitle: workspace.snapshotTitle,
              });
              updateWorkspace(active.path, (draft) => {
                draft.snapshotTitle = "";
                draft.route = { kind: "overview" };
              });
              setNotice(active.path, "overview", {
                tone: result.ignoredSources.length > 0 ? "warning" : "success",
                message: `Snapshot added: ${result.fileCount} files, ${formatBytes(result.byteCount)}.${
                  result.ignoredSources.length > 0 ? ` Ignored ${result.ignoredSources.length} duplicate or nested sources.` : ""
                }`,
              });
              await refreshSnapshots(active.path, false);
              closeWorkspaceModal();
              return true;
            } catch (error) {
              setNotice(active.path, "add", { tone: "error", message: String(error) });
              return false;
            } finally {
              setBusy(undefined);
            }
          }}
        />
      </WorkspaceModalView>
    ) : active && workspace && workspaceModal === "export" ? (
      <WorkspaceModalView title="Export Repository" onClose={closeWorkspaceModal}>
        <ExportRepositoryPage
          repository={active}
          draft={workspace}
          notice={workspaceNotice("export")}
          busy={busy === "export"}
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
              return true;
            } catch (error) {
              setNotice(active.path, "export", { tone: "error", message: String(error) });
              return false;
            } finally {
              setBusy(undefined);
            }
          }}
        />
      </WorkspaceModalView>
    ) : active && workspace && workspaceModal === "settings" ? (
      <WorkspaceModalView title="Repository Settings" onClose={closeWorkspaceModal}>
        <RepositorySettingsPage
          repository={active}
          notice={workspaceNotice("overview")}
          busy={busy === "settings" || busy === "delete-repository"}
          repositoryUnlocked={active.encryptionAlgorithm === "none" || Boolean(repositoryPasswords[active.path])}
          onRename={async (name) => {
            setBusy("settings");
            setNotice(active.path, "overview", { tone: "info", message: "Saving repository name..." });
            try {
              const info = await repositoryApi.rename(active.path, name);
              updateState((draft) => {
                const repository = repositoryByPath(draft, active.path);
                if (repository) repository.name = info.name;
              });
              setNotice(active.path, "overview", { tone: "success", message: "Repository name updated." });
              return true;
            } catch (error) {
              setNotice(active.path, "overview", { tone: "error", message: String(error) });
              return false;
            } finally {
              setBusy(undefined);
            }
          }}
          onUnlock={async (password) => {
            setBusy("settings");
            setNotice(active.path, "overview", { tone: "info", message: "Unlocking repository..." });
            try {
              await repositoryApi.unlock(active.path, password);
              setRepositoryPasswords((current) => ({ ...current, [active.path]: password }));
              setNotice(active.path, "overview", { tone: "success", message: "Repository unlocked." });
              return true;
            } catch (error) {
              setNotice(active.path, "overview", { tone: "error", message: String(error) });
              return false;
            } finally {
              setBusy(undefined);
            }
          }}
          onChangePassword={async (oldPassword, newPassword) => {
            setBusy("settings");
            setNotice(active.path, "overview", { tone: "info", message: "Changing repository password..." });
            try {
              await repositoryApi.changePassword(active.path, oldPassword, newPassword);
              setRepositoryPasswords((current) => ({ ...current, [active.path]: newPassword }));
              setNotice(active.path, "overview", { tone: "success", message: "Repository password changed." });
              return true;
            } catch (error) {
              setNotice(active.path, "overview", { tone: "error", message: String(error) });
              return false;
            } finally {
              setBusy(undefined);
            }
          }}
          onDelete={async (password) => {
            setBusy("delete-repository");
            setNotice(active.path, "overview", { tone: "info", message: "Deleting repository..." });
            try {
              await repositoryApi.delete(active.path, password);
              const removedPath = active.path;
              updateState((draft) => {
                draft.repositories = draft.repositories.filter((repository) => repository.path !== removedPath);
                delete draft.workspaces[removedPath];
                if (draft.activeRepositoryPath === removedPath) draft.activeRepositoryPath = undefined;
              });
              setSnapshots((current) => {
                const next = { ...current };
                delete next[removedPath];
                return next;
              });
              setRepositoryPasswords((current) => {
                const next = { ...current };
                delete next[removedPath];
                return next;
              });
              setWorkspaceModal(null);
              setGlobalPage(null);
              return true;
            } catch (error) {
              setNotice(active.path, "overview", { tone: "error", message: String(error) });
              return false;
            } finally {
              setBusy(undefined);
            }
          }}
        />
      </WorkspaceModalView>
    ) : active && workspace && workspaceModal === "restore" && workspace.route.kind === "restore" ? (
      (() => {
        const restoreRoute = workspace.route;
        const snapshot = snapshots[active.path]?.find((item) => item.id === restoreRoute.snapshotId);
        if (!snapshot) return null;
        return (
          <WorkspaceModalView title="Restore Snapshot" onClose={closeWorkspaceModal}>
            <RestoreSnapshotPage
              repository={active}
              snapshot={snapshot}
              draft={workspace}
              notice={workspaceNotice("restore")}
              busy={busy === "restore"}
              decryptionPassword={decryptionPassword}
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
                  return true;
                } catch (error) {
                  setNotice(active.path, "restore", { tone: "error", message: String(error) });
                  return false;
                } finally {
                  setBusy(undefined);
                }
              }}
            />
          </WorkspaceModalView>
        );
      })()
    ) : active && workspaceModal === "deleteSnapshot" && pendingDeleteSnapshot ? (
      <WorkspaceModalView title="Delete Snapshot" onClose={() => {
        setPendingDeleteSnapshot(undefined);
        setWorkspaceModal(null);
      }}>
        <DeleteSnapshotPage
          snapshot={pendingDeleteSnapshot}
          notice={workspaceNotice("overview")}
          busy={busy === "delete-snapshot"}
          onCancel={() => {
            setPendingDeleteSnapshot(undefined);
            setWorkspaceModal(null);
          }}
          onSubmit={async (password) => {
            setBusy("delete-snapshot");
            setBusySnapshotId(pendingDeleteSnapshot.id);
            setNotice(active.path, "overview", { tone: "info", message: "Deleting snapshot..." });
            try {
              const result = await repositoryApi.deleteSnapshot(active.path, pendingDeleteSnapshot.id, password);
              const warning = result.warnings.length > 0 ? ` ${result.warnings.join(" ")}` : "";
              setNotice(active.path, "overview", {
                tone: result.warnings.length > 0 ? "warning" : "success",
                message: `Snapshot deleted. Removed ${result.deletedObjectCount} objects and reclaimed ${formatBytes(result.reclaimedBytes)}.${warning}`,
              });
              await refreshSnapshots(active.path, false);
              setPendingDeleteSnapshot(undefined);
              setWorkspaceModal(null);
              return true;
            } catch (error) {
              setNotice(active.path, "overview", { tone: "error", message: String(error) });
              return false;
            } finally {
              setBusy(undefined);
              setBusySnapshotId(undefined);
            }
          }}
        />
      </WorkspaceModalView>
    ) : null;

  return (
    <main
      id="app-shell"
      className={[
        isResizing ? "is-resizing" : "",
        state.sidebarCollapsed ? "is-sidebar-collapsed" : "",
      ]
        .filter(Boolean)
        .join(" ")}
    >
      <AppTitleBar sidebarCollapsed={state.sidebarCollapsed} onToggleSidebar={toggleSidebarCollapsed} />
      <div id="app-content">
        {!state.sidebarCollapsed ? (
          <>
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
              onToggleSection={toggleSidebarSection}
              onReorder={reorderRepositorySection}
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
          </>
        ) : null}
        <section id="workspace" className="shadow" aria-label="Workspace">
          {workspaceContent}
        </section>
      </div>
      {modalContent}
    </main>
  );
}
