import type {
  AppState,
  RepositoryRecord,
  RepositoryWorkspace,
  SnapshotInfo,
} from "./types";
import { visibleRepositories } from "./state";

export type Notice = { tone: "info" | "success" | "error" | "warning"; message: string };

function required<T extends Element>(root: ParentNode, selector: string): T {
  const element = root.querySelector<T>(selector);
  if (!element) throw new Error(`Missing rendered element: ${selector}`);
  return element;
}

function setNotice(root: ParentNode, notice?: Notice): void {
  const element = root.querySelector<HTMLElement>(".page-notice");
  if (!element) return;
  element.textContent = notice?.message ?? "";
  element.dataset.tone = notice?.tone ?? "info";
  element.hidden = !notice?.message;
}

function setValue(root: ParentNode, selector: string, value: string): void {
  required<HTMLInputElement | HTMLSelectElement>(root, selector).value = value;
}

function pageHeader(root: HTMLElement, title: string, subtitle: string): void {
  required<HTMLElement>(root, ".page-title").textContent = title;
  const subtitleElement = required<HTMLElement>(root, ".page-subtitle");
  subtitleElement.textContent = subtitle;
  subtitleElement.title = subtitle;
}

function secondaryPageTemplate(): string {
  return `
    <header class="page-bar">
      <button class="compact-button back-button" type="button" aria-label="Back">Back</button>
      <div class="page-heading">
        <h1 class="page-title"></h1>
        <p class="page-subtitle"></p>
      </div>
    </header>
    <div class="page-content"></div>
  `;
}

export function renderSidebar(
  root: HTMLElement,
  state: AppState,
  unavailable: ReadonlySet<string>,
  globalPageActive: boolean,
): void {
  root.replaceChildren();

  const header = document.createElement("header");
  header.className = "sidebar-header";
  const brand = document.createElement("h1");
  brand.textContent = "BackupTool";
  header.append(brand);

  const actions = document.createElement("div");
  actions.className = "sidebar-actions";
  for (const [id, label] of [
    ["new-repository", "New"],
    ["open-repository", "Open"],
    ["import-repository-page", "Import"],
  ]) {
    const button = document.createElement("button");
    button.id = id;
    button.type = "button";
    button.className = "sidebar-action";
    button.textContent = label;
    actions.append(button);
  }

  const sectionTitle = document.createElement("div");
  sectionTitle.className = "sidebar-section-title";
  sectionTitle.textContent = "Repositories";

  const list = document.createElement("div");
  list.className = "repository-list";
  list.id = "repository-list";
  const repositories = visibleRepositories(state);
  if (repositories.length === 0) {
    const empty = document.createElement("p");
    empty.className = "sidebar-empty";
    empty.textContent = "No repositories opened";
    list.append(empty);
  }

  for (const repository of repositories) {
    const row = document.createElement("div");
    row.className = "repository-row";
    row.dataset.repositoryPath = repository.path;
    row.tabIndex = 0;
    row.title = repository.path;
    if (!globalPageActive && repository.path === state.activeRepositoryPath) {
      row.classList.add("is-active");
    }
    if (unavailable.has(repository.path)) row.classList.add("is-unavailable");

    const details = document.createElement("div");
    details.className = "repository-details";
    const name = document.createElement("span");
    name.className = "repository-name";
    name.textContent = repository.name;
    const path = document.createElement("span");
    path.className = "repository-path";
    path.textContent = unavailable.has(repository.path) ? "Unavailable" : repository.path;
    details.append(name, path);

    const rowActions = document.createElement("div");
    rowActions.className = "repository-actions";
    const pin = document.createElement("button");
    pin.type = "button";
    pin.className = "row-action";
    pin.dataset.action = "pin";
    pin.textContent = repository.pinned ? "Unpin" : "Pin";
    const archive = document.createElement("button");
    archive.type = "button";
    archive.className = "row-action";
    archive.dataset.action = "archive";
    archive.textContent = "Archive";
    rowActions.append(pin, archive);
    row.append(details, rowActions);
    list.append(row);
  }

  root.append(header, actions, sectionTitle, list);
}

export function renderEmptyWorkspace(root: HTMLElement): void {
  root.innerHTML = `
    <div class="empty-workspace">
      <div>
        <p class="eyebrow">Repository workspace</p>
        <h1>Open a backup repository</h1>
        <p>Select an existing repository, create a new one, or import a tar archive.</p>
      </div>
    </div>
  `;
}

export function renderNewRepository(root: HTMLElement, notice?: Notice): void {
  root.innerHTML = secondaryPageTemplate();
  pageHeader(root, "New Repository", "Create a repository directory in a selected location");
  required<HTMLElement>(root, ".page-content").innerHTML = `
    <form id="new-repository-form" class="form-panel">
      <label>Parent directory
        <span class="path-control">
          <input id="new-parent-path" type="text" autocomplete="off" />
          <button id="browse-new-parent" type="button" class="secondary-button">Browse</button>
        </span>
      </label>
      <label>Repository name
        <input id="new-repository-name" type="text" autocomplete="off" maxlength="120" />
      </label>
      <div class="form-actions">
        <button id="create-repository" type="submit" class="primary-button">Create Repository</button>
      </div>
      <p class="page-notice" aria-live="polite"></p>
    </form>
  `;
  setNotice(root, notice);
}

export function renderImportRepository(root: HTMLElement, notice?: Notice): void {
  root.innerHTML = secondaryPageTemplate();
  pageHeader(root, "Import Repository", "Import a tar archive into an empty directory");
  required<HTMLElement>(root, ".page-content").innerHTML = `
    <form id="import-repository-form" class="form-panel">
      <label>Archive algorithm
        <select id="import-algorithm" disabled><option value="tar">tar</option></select>
      </label>
      <label>Archive file
        <span class="path-control">
          <input id="import-archive-path" type="text" autocomplete="off" />
          <button id="browse-import-archive" type="button" class="secondary-button">Browse</button>
        </span>
      </label>
      <label>Destination directory
        <span class="path-control">
          <input id="import-destination" type="text" autocomplete="off" />
          <button id="browse-import-destination" type="button" class="secondary-button">Browse</button>
        </span>
      </label>
      <div class="form-actions">
        <button id="run-import" type="submit" class="primary-button">Import Repository</button>
      </div>
      <p class="page-notice" aria-live="polite"></p>
    </form>
  `;
  setNotice(root, notice);
}

function formatSnapshotTime(snapshot: SnapshotInfo): string {
  return snapshot.createdUnixSeconds
    ? new Date(snapshot.createdUnixSeconds * 1000).toLocaleString()
    : "Unknown time";
}

export function formatBytes(value: number): string {
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

export function renderOverview(
  root: HTMLElement,
  repository: RepositoryRecord,
  snapshots: SnapshotInfo[] | undefined,
  notice?: Notice,
): void {
  root.innerHTML = `
    <header class="workspace-header">
      <div class="workspace-identity">
        <p class="eyebrow">Repository</p>
        <h1 id="workspace-repository-name"></h1>
        <p id="workspace-repository-path"></p>
      </div>
      <div class="workspace-actions">
        <button id="add-snapshot-page" type="button" class="primary-button">Add Snapshot</button>
        <button id="export-repository-page" type="button" class="secondary-button">Export Repository</button>
      </div>
    </header>
    <div class="overview-content">
      <div class="section-heading">
        <div><p class="eyebrow">History</p><h2>Snapshots</h2></div>
        <button id="refresh-snapshots" type="button" class="compact-button">Refresh</button>
      </div>
      <p class="page-notice" aria-live="polite"></p>
      <div id="snapshot-list" class="snapshot-list"></div>
    </div>
  `;
  required<HTMLElement>(root, "#workspace-repository-name").textContent = repository.name;
  const path = required<HTMLElement>(root, "#workspace-repository-path");
  path.textContent = repository.path;
  path.title = repository.path;
  setNotice(root, notice);
  const list = required<HTMLElement>(root, "#snapshot-list");
  if (!snapshots) {
    list.innerHTML = '<p class="snapshot-empty">Loading snapshots…</p>';
    return;
  }
  if (snapshots.length === 0) {
    list.innerHTML = '<p class="snapshot-empty">No snapshots in this repository</p>';
    return;
  }
  for (const snapshot of snapshots) {
    const row = document.createElement("article");
    row.className = "snapshot-row";
    row.dataset.snapshotId = snapshot.id;
    row.tabIndex = 0;
    const details = document.createElement("div");
    details.className = "snapshot-details";
    const title = document.createElement("h3");
    title.textContent = snapshot.title?.trim() || "Untitled";
    const time = document.createElement("p");
    time.textContent = `${formatSnapshotTime(snapshot)} · ${snapshot.fileCount} files · ${formatBytes(snapshot.byteCount)}`;
    const id = document.createElement("code");
    id.textContent = snapshot.id;
    details.append(title, time, id);
    const deleteButton = document.createElement("button");
    deleteButton.type = "button";
    deleteButton.className = "danger-button snapshot-delete";
    deleteButton.dataset.action = "delete";
    deleteButton.textContent = "Delete";
    row.append(details, deleteButton);
    list.append(row);
  }
}

export function renderAddSnapshot(
  root: HTMLElement,
  repository: RepositoryRecord,
  draft: RepositoryWorkspace,
  notice?: Notice,
): void {
  root.innerHTML = secondaryPageTemplate();
  pageHeader(root, "Add Snapshot", repository.path);
  required<HTMLElement>(root, ".page-content").innerHTML = `
    <form id="backup-form" class="form-panel wide-form">
      <div class="field-group">
        <div class="group-heading"><div><h2>Source directories</h2><p>Add one or more directories to this snapshot.</p></div><button id="add-source" type="button" class="secondary-button">Add</button></div>
        <div id="source-list" class="source-list"></div>
      </div>
      <div class="form-grid two-columns">
        <label>Compression algorithm
          <select id="compression-algorithm"><option value="none">None</option><option value="zstd">zstd</option></select>
        </label>
        <label>Encryption algorithm
          <select id="encryption-algorithm"><option value="none">None</option><option value="aes-256-gcm">AES-256-GCM</option></select>
        </label>
        <label id="encryption-password-field" hidden>Encryption password
          <input id="encryption-password" type="password" autocomplete="new-password" />
        </label>
        <label>Snapshot title
          <input id="snapshot-title" type="text" maxlength="120" autocomplete="off" />
        </label>
      </div>
      <details id="filter-details" class="advanced-panel">
        <summary>Advanced filters</summary>
        <div class="form-grid three-columns">
          <label>Include path contains<input id="include-path" type="text" autocomplete="off" /></label>
          <label>Exclude path contains<input id="exclude-path" type="text" autocomplete="off" /></label>
          <label>Extensions<input id="extensions" type="text" autocomplete="off" placeholder="txt;png" /></label>
          <label>Include file name contains<input id="include-name" type="text" autocomplete="off" /></label>
          <label>Exclude file name contains<input id="exclude-name" type="text" autocomplete="off" /></label>
          <label>Minimum size<input id="min-size" type="number" min="0" step="1" /></label>
          <label>Maximum size<input id="max-size" type="number" min="0" step="1" /></label>
          <label>Modified after<input id="modified-after" type="datetime-local" /></label>
          <label>Modified before<input id="modified-before" type="datetime-local" /></label>
        </div>
      </details>
      <div class="form-actions"><button id="run-backup" type="submit" class="primary-button">Add Snapshot</button></div>
      <p class="page-notice" aria-live="polite"></p>
    </form>
  `;
  setValue(root, "#compression-algorithm", draft.compressionAlgorithm);
  setValue(root, "#encryption-algorithm", draft.encryptionAlgorithm);
  setValue(root, "#snapshot-title", draft.snapshotTitle);
  setValue(root, "#include-path", draft.filter.includePath);
  setValue(root, "#exclude-path", draft.filter.excludePath);
  setValue(root, "#extensions", draft.filter.extensions);
  setValue(root, "#include-name", draft.filter.includeName);
  setValue(root, "#exclude-name", draft.filter.excludeName);
  setValue(root, "#min-size", draft.filter.minSize);
  setValue(root, "#max-size", draft.filter.maxSize);
  setValue(root, "#modified-after", draft.filter.modifiedAfter);
  setValue(root, "#modified-before", draft.filter.modifiedBefore);
  required<HTMLDetailsElement>(root, "#filter-details").open = draft.filtersOpen;
  required<HTMLElement>(root, "#encryption-password-field").hidden =
    draft.encryptionAlgorithm !== "aes-256-gcm";
  renderSourcePaths(required<HTMLElement>(root, "#source-list"), draft.sourcePaths);
  setNotice(root, notice);
}

export function renderSourcePaths(root: HTMLElement, sourcePaths: readonly string[]): void {
  root.replaceChildren();
  if (sourcePaths.length === 0) {
    const empty = document.createElement("p");
    empty.className = "source-empty";
    empty.textContent = "No source directories added";
    root.append(empty);
    return;
  }
  sourcePaths.forEach((source, index) => {
    const row = document.createElement("div");
    row.className = "source-row";
    const text = document.createElement("span");
    text.textContent = source;
    text.title = source;
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "compact-button";
    remove.dataset.removeSource = String(index);
    remove.textContent = "Remove";
    row.append(text, remove);
    root.append(row);
  });
}

export function renderExportRepository(
  root: HTMLElement,
  repository: RepositoryRecord,
  draft: RepositoryWorkspace,
  notice?: Notice,
): void {
  root.innerHTML = secondaryPageTemplate();
  pageHeader(root, "Export Repository", repository.path);
  required<HTMLElement>(root, ".page-content").innerHTML = `
    <form id="export-form" class="form-panel">
      <label>Archive algorithm<select id="export-algorithm" disabled><option value="tar">tar</option></select></label>
      <label>Export file
        <span class="path-control"><input id="export-path" type="text" autocomplete="off" /><button id="browse-export-path" type="button" class="secondary-button">Browse</button></span>
      </label>
      <div class="form-actions"><button id="run-export" type="submit" class="primary-button">Export Repository</button></div>
      <p class="page-notice" aria-live="polite"></p>
    </form>
  `;
  setValue(root, "#export-path", draft.exportPath);
  setNotice(root, notice);
}

export function renderRestoreSnapshot(
  root: HTMLElement,
  repository: RepositoryRecord,
  snapshot: SnapshotInfo,
  draft: RepositoryWorkspace,
  notice?: Notice,
): void {
  root.innerHTML = secondaryPageTemplate();
  pageHeader(root, "Restore Snapshot", snapshot.title?.trim() || "Untitled");
  required<HTMLElement>(root, ".page-content").innerHTML = `
    <form id="restore-form" class="form-panel">
      <div class="snapshot-summary"><span>Repository</span><strong id="restore-repository"></strong><span>Created</span><strong id="restore-created"></strong><span>Snapshot ID</span><code id="restore-snapshot-id"></code></div>
      <label>Restore directory
        <span class="path-control"><input id="restore-destination" type="text" autocomplete="off" /><button id="browse-restore-destination" type="button" class="secondary-button">Browse</button></span>
      </label>
      <label>Restore path strategy
        <select id="restore-path-strategy"><option value="preserveRelativePath">Preserve Relative Path</option><option value="preserveFullPath">Preserve Full Path</option><option value="flatten">Flatten</option></select>
      </label>
      <label id="flatten-conflict-field">Flatten conflict strategy
        <select id="flatten-conflict-strategy"><option value="rename">Rename</option><option value="error">Error</option><option value="skip">Skip</option><option value="overwrite">Overwrite</option></select>
      </label>
      <label>Decryption password
        <input id="decryption-password" type="password" autocomplete="current-password" />
      </label>
      <div class="form-actions"><button id="run-restore" type="submit" class="primary-button">Restore Snapshot</button></div>
      <p class="page-notice" aria-live="polite"></p>
    </form>
  `;
  required<HTMLElement>(root, "#restore-repository").textContent = repository.name;
  required<HTMLElement>(root, "#restore-created").textContent = formatSnapshotTime(snapshot);
  required<HTMLElement>(root, "#restore-snapshot-id").textContent = snapshot.id;
  setValue(root, "#restore-destination", draft.restoreDestination);
  setValue(root, "#restore-path-strategy", draft.restorePathStrategy);
  setValue(root, "#flatten-conflict-strategy", draft.flattenConflictStrategy);
  required<HTMLElement>(root, "#flatten-conflict-field").hidden =
    draft.restorePathStrategy !== "flatten";
  setNotice(root, notice);
}
