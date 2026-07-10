import "./styles.css";
import {
  chooseDirectory,
  chooseExportPath,
  chooseTarArchive,
  confirmSnapshotDeletion,
  repositoryApi,
} from "./api";
import {
  activeRepository as findActiveRepository,
  repositoryByPath as findRepositoryByPath,
  updateWorkspaceRoute,
} from "./routing";
import {
  ensureWorkspace,
  loadState,
  reconcileSnapshotRoute,
  scheduleStateSave,
  upsertRepository,
} from "./state";
import type {
  AppState,
  RepositoryRecord,
  RepositoryWorkspace,
  SnapshotInfo,
  WorkspaceRoute,
} from "./types";
import {
  formatBytes,
  type Notice,
  renderAddSnapshot,
  renderEmptyWorkspace,
  renderExportRepository,
  renderImportRepository,
  renderNewRepository,
  renderOverview,
  renderRestoreSnapshot,
  renderSidebar,
  renderSourcePaths,
} from "./views";

type GlobalPage = "new" | "import" | null;

const sidebar = required<HTMLElement>(document, "#repository-sidebar");
const workspaceRoot = required<HTMLElement>(document, "#workspace");
const resizer = required<HTMLElement>(document, "#sidebar-resizer");
const snapshotsByRepository = new Map<string, SnapshotInfo[]>();
const unavailableRepositories = new Set<string>();
const notices = new Map<string, Notice>();
let state: AppState;
let globalPage: GlobalPage = null;
let globalNotice: Notice | undefined;

function required<T extends Element>(root: ParentNode, selector: string): T {
  const element = root.querySelector<T>(selector);
  if (!element) throw new Error(`Missing required element: ${selector}`);
  return element;
}

function activeRepository(): RepositoryRecord | undefined {
  return findActiveRepository(state);
}

function noticeKey(path: string, route: WorkspaceRoute["kind"]): string {
  return `${path}\n${route}`;
}

function getNotice(path: string, route: WorkspaceRoute["kind"]): Notice | undefined {
  return notices.get(noticeKey(path, route));
}

function setNotice(path: string, route: WorkspaceRoute["kind"], notice: Notice): void {
  notices.set(noticeKey(path, route), notice);
}

function updateVisibleNotice(notice: Notice): void {
  const element = workspaceRoot.querySelector<HTMLElement>(".page-notice");
  if (!element) return;
  element.hidden = false;
  element.dataset.tone = notice.tone;
  element.textContent = notice.message;
}

function setFormBusy(form: HTMLFormElement, busy: boolean): void {
  for (const element of form.elements) {
    if (element instanceof HTMLButtonElement || element instanceof HTMLInputElement || element instanceof HTMLSelectElement) {
      if (busy) {
        element.dataset.disabledBeforeBusy = String(element.disabled);
        element.disabled = true;
      } else {
        element.disabled = element.dataset.disabledBeforeBusy === "true";
        delete element.dataset.disabledBeforeBusy;
      }
    }
  }
  form.dataset.busy = String(busy);
}

function persist(): void {
  scheduleStateSave(state);
}

function setRoute(route: WorkspaceRoute): void {
  const repository = activeRepository();
  if (!repository) return;
  updateWorkspaceRoute(state, repository.path, route);
  persist();
  renderWorkspace();
}

function renderSidebarOnly(): void {
  renderSidebar(sidebar, state, unavailableRepositories, globalPage !== null);
  bindSidebar();
}

function renderAll(): void {
  document.documentElement.style.setProperty("--sidebar-width", `${state.sidebarWidth}px`);
  renderSidebarOnly();
  renderWorkspace();
}

function renderWorkspace(): void {
  if (globalPage === "new") {
    renderNewRepository(workspaceRoot, globalNotice);
    bindNewRepositoryPage();
    return;
  }
  if (globalPage === "import") {
    renderImportRepository(workspaceRoot, globalNotice);
    bindImportRepositoryPage();
    return;
  }

  const repository = activeRepository();
  if (!repository || repository.archived) {
    renderEmptyWorkspace(workspaceRoot);
    return;
  }
  const draft = ensureWorkspace(state, repository.path);
  const snapshots = snapshotsByRepository.get(repository.path);
  if (snapshots && reconcileSnapshotRoute(draft, new Set(snapshots.map((item) => item.id)))) {
    persist();
  }

  switch (draft.route.kind) {
    case "overview":
      renderOverview(
        workspaceRoot,
        repository,
        snapshots,
        getNotice(repository.path, "overview"),
      );
      bindOverview(repository);
      break;
    case "add":
      renderAddSnapshot(
        workspaceRoot,
        repository,
        draft,
        getNotice(repository.path, "add"),
      );
      bindAddSnapshot(repository, draft);
      break;
    case "export":
      renderExportRepository(
        workspaceRoot,
        repository,
        draft,
        getNotice(repository.path, "export"),
      );
      bindExportRepository(repository, draft);
      break;
    case "restore": {
      const snapshotId = draft.route.snapshotId;
      const snapshot = snapshots?.find((item) => item.id === snapshotId);
      if (!snapshot) {
        draft.route = { kind: "overview" };
        persist();
        renderWorkspace();
        return;
      }
      renderRestoreSnapshot(
        workspaceRoot,
        repository,
        snapshot,
        draft,
        getNotice(repository.path, "restore"),
      );
      bindRestoreSnapshot(repository, snapshot, draft);
      break;
    }
  }
}

function bindSidebar(): void {
  required<HTMLButtonElement>(sidebar, "#new-repository").addEventListener("click", () => {
    globalPage = "new";
    globalNotice = undefined;
    renderAll();
  });
  required<HTMLButtonElement>(sidebar, "#import-repository-page").addEventListener("click", () => {
    globalPage = "import";
    globalNotice = undefined;
    renderAll();
  });
  required<HTMLButtonElement>(sidebar, "#open-repository").addEventListener("click", async () => {
    try {
      const selected = await chooseDirectory("Open repository");
      if (selected) await openAndActivateRepository(selected);
    } catch (error) {
      globalPage = null;
      renderWorkspace();
      showOverviewError(String(error));
    }
  });

  for (const row of sidebar.querySelectorAll<HTMLElement>(".repository-row")) {
    const path = row.dataset.repositoryPath;
    if (!path) continue;
    const activate = (): void => {
      void openAndActivateRepository(path);
    };
    row.addEventListener("click", (event) => {
      if (!(event.target instanceof HTMLButtonElement)) activate();
    });
    row.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        activate();
      }
    });
    row.querySelector<HTMLButtonElement>('[data-action="pin"]')?.addEventListener("click", (event) => {
      event.stopPropagation();
      const repository = findRepositoryByPath(state, path);
      if (!repository) return;
      repository.pinned = !repository.pinned;
      persist();
      renderSidebarOnly();
    });
    row.querySelector<HTMLButtonElement>('[data-action="archive"]')?.addEventListener("click", (event) => {
      event.stopPropagation();
      const repository = findRepositoryByPath(state, path);
      if (!repository) return;
      repository.archived = true;
      if (state.activeRepositoryPath === path) state.activeRepositoryPath = undefined;
      globalPage = null;
      persist();
      renderAll();
    });
  }
}

async function openAndActivateRepository(path: string): Promise<void> {
  try {
    const info = await repositoryApi.open(path);
    const repository = upsertRepository(state, info);
    unavailableRepositories.delete(path);
    unavailableRepositories.delete(repository.path);
    globalPage = null;
    persist();
    renderAll();
    await refreshSnapshots(repository.path, false);
  } catch (error) {
    unavailableRepositories.add(path);
    renderSidebarOnly();
    showOverviewError(String(error));
  }
}

function showOverviewError(message: string): void {
  const repository = activeRepository();
  if (repository) setNotice(repository.path, "overview", { tone: "error", message });
  updateVisibleNotice({ tone: "error", message });
}

async function refreshSnapshots(path: string, announce = true): Promise<void> {
  try {
    const snapshots = await repositoryApi.listSnapshots(path);
    snapshotsByRepository.set(path, snapshots);
    unavailableRepositories.delete(path);
    const draft = ensureWorkspace(state, path);
    if (reconcileSnapshotRoute(draft, new Set(snapshots.map((item) => item.id)))) {
      persist();
    }
    if (announce) {
      setNotice(path, "overview", {
        tone: "success",
        message: `Loaded ${snapshots.length} snapshot${snapshots.length === 1 ? "" : "s"}.`,
      });
    }
  } catch (error) {
    unavailableRepositories.add(path);
    setNotice(path, "overview", { tone: "error", message: String(error) });
  }
  renderAll();
}

function bindNewRepositoryPage(): void {
  workspaceRoot.querySelector<HTMLButtonElement>(".back-button")?.addEventListener("click", closeGlobalPage);
  required<HTMLButtonElement>(workspaceRoot, "#browse-new-parent").addEventListener("click", async () => {
    const selected = await chooseDirectory("Choose repository parent");
    if (selected) required<HTMLInputElement>(workspaceRoot, "#new-parent-path").value = selected;
  });
  const form = required<HTMLFormElement>(workspaceRoot, "#new-repository-form");
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    setFormBusy(form, true);
    updateVisibleNotice({ tone: "info", message: "Creating repository…" });
    try {
      const info = await repositoryApi.create(
        required<HTMLInputElement>(form, "#new-parent-path").value,
        required<HTMLInputElement>(form, "#new-repository-name").value,
      );
      const repository = upsertRepository(state, info);
      globalPage = null;
      persist();
      setNotice(repository.path, "overview", {
        tone: "success",
        message: "Repository created.",
      });
      await refreshSnapshots(repository.path, false);
    } catch (error) {
      globalNotice = { tone: "error", message: String(error) };
      setFormBusy(form, false);
      updateVisibleNotice(globalNotice);
    }
  });
}

function bindImportRepositoryPage(): void {
  workspaceRoot.querySelector<HTMLButtonElement>(".back-button")?.addEventListener("click", closeGlobalPage);
  required<HTMLButtonElement>(workspaceRoot, "#browse-import-archive").addEventListener("click", async () => {
    const selected = await chooseTarArchive();
    if (selected) required<HTMLInputElement>(workspaceRoot, "#import-archive-path").value = selected;
  });
  required<HTMLButtonElement>(workspaceRoot, "#browse-import-destination").addEventListener("click", async () => {
    const selected = await chooseDirectory("Choose import destination");
    if (selected) required<HTMLInputElement>(workspaceRoot, "#import-destination").value = selected;
  });
  const form = required<HTMLFormElement>(workspaceRoot, "#import-repository-form");
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    setFormBusy(form, true);
    updateVisibleNotice({ tone: "info", message: "Importing repository…" });
    try {
      const result = await repositoryApi.import(
        required<HTMLInputElement>(form, "#import-archive-path").value,
        required<HTMLInputElement>(form, "#import-destination").value,
      );
      const info = await repositoryApi.open(result.path);
      const repository = upsertRepository(state, info);
      globalPage = null;
      persist();
      setNotice(repository.path, "overview", {
        tone: "success",
        message: `Repository imported (${formatBytes(result.byteCount)}).`,
      });
      await refreshSnapshots(repository.path, false);
    } catch (error) {
      globalNotice = { tone: "error", message: String(error) };
      setFormBusy(form, false);
      updateVisibleNotice(globalNotice);
    }
  });
}

function closeGlobalPage(): void {
  globalPage = null;
  globalNotice = undefined;
  renderAll();
}

function bindOverview(repository: RepositoryRecord): void {
  required<HTMLButtonElement>(workspaceRoot, "#add-snapshot-page").addEventListener("click", () => {
    setRoute({ kind: "add" });
  });
  required<HTMLButtonElement>(workspaceRoot, "#export-repository-page").addEventListener("click", () => {
    setRoute({ kind: "export" });
  });
  required<HTMLButtonElement>(workspaceRoot, "#refresh-snapshots").addEventListener("click", () => {
    void refreshSnapshots(repository.path);
  });
  for (const row of workspaceRoot.querySelectorAll<HTMLElement>(".snapshot-row")) {
    const snapshotId = row.dataset.snapshotId;
    if (!snapshotId) continue;
    const openRestore = (): void => setRoute({ kind: "restore", snapshotId });
    row.addEventListener("click", (event) => {
      if (!(event.target instanceof HTMLButtonElement)) openRestore();
    });
    row.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        openRestore();
      }
    });
    row.querySelector<HTMLButtonElement>('[data-action="delete"]')?.addEventListener("click", async (event) => {
      event.stopPropagation();
      const snapshot = snapshotsByRepository
        .get(repository.path)
        ?.find((item) => item.id === snapshotId);
      if (!snapshot || !(await confirmSnapshotDeletion(snapshot.title?.trim() || "Untitled"))) return;
      const button = event.currentTarget as HTMLButtonElement;
      button.disabled = true;
      try {
        const result = await repositoryApi.deleteSnapshot(repository.path, snapshotId);
        const warning = result.warnings.length > 0 ? ` ${result.warnings.join(" ")}` : "";
        setNotice(repository.path, "overview", {
          tone: result.warnings.length > 0 ? "warning" : "success",
          message: `Snapshot deleted. Removed ${result.deletedObjectCount} objects and reclaimed ${formatBytes(result.reclaimedBytes)}.${warning}`,
        });
        await refreshSnapshots(repository.path, false);
      } catch (error) {
        button.disabled = false;
        setNotice(repository.path, "overview", { tone: "error", message: String(error) });
        updateVisibleNotice({ tone: "error", message: String(error) });
      }
    });
  }
}

function bindBackToOverview(): void {
  workspaceRoot.querySelector<HTMLButtonElement>(".back-button")?.addEventListener("click", () => {
    setRoute({ kind: "overview" });
  });
}

function bindDraftInput(
  root: ParentNode,
  selector: string,
  update: (value: string) => void,
): void {
  required<HTMLInputElement | HTMLSelectElement>(root, selector).addEventListener("input", (event) => {
    update((event.currentTarget as HTMLInputElement | HTMLSelectElement).value);
    persist();
  });
}

function bindAddSnapshot(repository: RepositoryRecord, draft: RepositoryWorkspace): void {
  bindBackToOverview();
  required<HTMLButtonElement>(workspaceRoot, "#add-source").addEventListener("click", async () => {
    const selected = await chooseDirectory("Add source directory");
    if (!selected) return;
    const key = selected.replace(/\\/g, "/").toLocaleLowerCase();
    if (!draft.sourcePaths.some((path) => path.replace(/\\/g, "/").toLocaleLowerCase() === key)) {
      draft.sourcePaths.push(selected);
      persist();
      refreshSourceList(draft);
    }
  });
  bindSourceRemoval(draft);
  bindDraftInput(workspaceRoot, "#compression-algorithm", (value) => {
    draft.compressionAlgorithm = value === "zstd" ? "zstd" : "none";
  });
  const encryption = required<HTMLSelectElement>(workspaceRoot, "#encryption-algorithm");
  encryption.addEventListener("change", () => {
    draft.encryptionAlgorithm = encryption.value === "aes-256-gcm" ? "aes-256-gcm" : "none";
    const field = required<HTMLElement>(workspaceRoot, "#encryption-password-field");
    field.hidden = draft.encryptionAlgorithm !== "aes-256-gcm";
    if (field.hidden) required<HTMLInputElement>(field, "#encryption-password").value = "";
    persist();
  });
  bindDraftInput(workspaceRoot, "#snapshot-title", (value) => (draft.snapshotTitle = value));
  const filterBindings: Array<[string, keyof RepositoryWorkspace["filter"]]> = [
    ["#include-path", "includePath"],
    ["#exclude-path", "excludePath"],
    ["#extensions", "extensions"],
    ["#include-name", "includeName"],
    ["#exclude-name", "excludeName"],
    ["#min-size", "minSize"],
    ["#max-size", "maxSize"],
    ["#modified-after", "modifiedAfter"],
    ["#modified-before", "modifiedBefore"],
  ];
  for (const [selector, field] of filterBindings) {
    bindDraftInput(workspaceRoot, selector, (value) => (draft.filter[field] = value));
  }
  required<HTMLDetailsElement>(workspaceRoot, "#filter-details").addEventListener("toggle", (event) => {
    draft.filtersOpen = (event.currentTarget as HTMLDetailsElement).open;
    persist();
  });

  const form = required<HTMLFormElement>(workspaceRoot, "#backup-form");
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const password = required<HTMLInputElement>(form, "#encryption-password").value;
    const validation = validateBackupDraft(draft, password);
    if (validation) {
      updateVisibleNotice({ tone: "error", message: validation });
      return;
    }
    setFormBusy(form, true);
    updateVisibleNotice({ tone: "info", message: "Adding snapshot…" });
    try {
      const result = await repositoryApi.backup({
        repositoryPath: repository.path,
        sources: draft.sourcePaths,
        filter: draft.filter,
        compressionAlgorithm: draft.compressionAlgorithm,
        encryptionAlgorithm: draft.encryptionAlgorithm,
        encryptionPassword: password,
        snapshotTitle: draft.snapshotTitle,
      });
      draft.snapshotTitle = "";
      draft.route = { kind: "overview" };
      setNotice(repository.path, "overview", {
        tone: result.ignoredSources.length > 0 ? "warning" : "success",
        message: `Snapshot added: ${result.fileCount} files, ${formatBytes(result.byteCount)}.${
          result.ignoredSources.length > 0
            ? ` Ignored ${result.ignoredSources.length} duplicate or nested sources.`
            : ""
        }`,
      });
      persist();
      await refreshSnapshots(repository.path, false);
    } catch (error) {
      setFormBusy(form, false);
      setNotice(repository.path, "add", { tone: "error", message: String(error) });
      updateVisibleNotice({ tone: "error", message: String(error) });
    }
  });
}

function refreshSourceList(draft: RepositoryWorkspace): void {
  renderSourcePaths(required<HTMLElement>(workspaceRoot, "#source-list"), draft.sourcePaths);
  bindSourceRemoval(draft);
}

function bindSourceRemoval(draft: RepositoryWorkspace): void {
  for (const button of workspaceRoot.querySelectorAll<HTMLButtonElement>("[data-remove-source]")) {
    button.addEventListener("click", () => {
      draft.sourcePaths.splice(Number(button.dataset.removeSource), 1);
      persist();
      refreshSourceList(draft);
    });
  }
}

function validateBackupDraft(draft: RepositoryWorkspace, password: string): string | undefined {
  if (draft.sourcePaths.length === 0) return "Add at least one source directory.";
  if (draft.encryptionAlgorithm === "aes-256-gcm" && !password) {
    return "Encryption password must not be empty.";
  }
  const minimum = draft.filter.minSize ? Number(draft.filter.minSize) : undefined;
  const maximum = draft.filter.maxSize ? Number(draft.filter.maxSize) : undefined;
  if (minimum !== undefined && maximum !== undefined && minimum > maximum) {
    return "Minimum size must not exceed maximum size.";
  }
  if (
    draft.filter.modifiedAfter &&
    draft.filter.modifiedBefore &&
    new Date(draft.filter.modifiedAfter) > new Date(draft.filter.modifiedBefore)
  ) {
    return "Modified after must not be later than modified before.";
  }
  return undefined;
}

function bindExportRepository(repository: RepositoryRecord, draft: RepositoryWorkspace): void {
  bindBackToOverview();
  bindDraftInput(workspaceRoot, "#export-path", (value) => (draft.exportPath = value));
  required<HTMLButtonElement>(workspaceRoot, "#browse-export-path").addEventListener("click", async () => {
    const selected = await chooseExportPath(`${repository.name}.tar`);
    if (selected) {
      draft.exportPath = selected;
      required<HTMLInputElement>(workspaceRoot, "#export-path").value = selected;
      persist();
    }
  });
  const form = required<HTMLFormElement>(workspaceRoot, "#export-form");
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    setFormBusy(form, true);
    updateVisibleNotice({ tone: "info", message: "Exporting repository…" });
    try {
      const result = await repositoryApi.export(repository.path, draft.exportPath);
      setNotice(repository.path, "export", {
        tone: "success",
        message: `Repository exported to ${result.path} (${formatBytes(result.byteCount)}).`,
      });
      setFormBusy(form, false);
      updateVisibleNotice(getNotice(repository.path, "export")!);
    } catch (error) {
      setFormBusy(form, false);
      setNotice(repository.path, "export", { tone: "error", message: String(error) });
      updateVisibleNotice({ tone: "error", message: String(error) });
    }
  });
}

function bindRestoreSnapshot(
  repository: RepositoryRecord,
  snapshot: SnapshotInfo,
  draft: RepositoryWorkspace,
): void {
  bindBackToOverview();
  bindDraftInput(workspaceRoot, "#restore-destination", (value) => {
    draft.restoreDestination = value;
  });
  required<HTMLButtonElement>(workspaceRoot, "#browse-restore-destination").addEventListener("click", async () => {
    const selected = await chooseDirectory("Choose restore destination");
    if (selected) {
      draft.restoreDestination = selected;
      required<HTMLInputElement>(workspaceRoot, "#restore-destination").value = selected;
      persist();
    }
  });
  const pathStrategy = required<HTMLSelectElement>(workspaceRoot, "#restore-path-strategy");
  pathStrategy.addEventListener("change", () => {
    draft.restorePathStrategy = pathStrategy.value as RepositoryWorkspace["restorePathStrategy"];
    required<HTMLElement>(workspaceRoot, "#flatten-conflict-field").hidden =
      draft.restorePathStrategy !== "flatten";
    persist();
  });
  bindDraftInput(workspaceRoot, "#flatten-conflict-strategy", (value) => {
    draft.flattenConflictStrategy = value as RepositoryWorkspace["flattenConflictStrategy"];
  });
  const form = required<HTMLFormElement>(workspaceRoot, "#restore-form");
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    setFormBusy(form, true);
    updateVisibleNotice({ tone: "info", message: "Restoring snapshot…" });
    try {
      const result = await repositoryApi.restore({
        repositoryPath: repository.path,
        snapshotId: snapshot.id,
        destination: draft.restoreDestination,
        pathStrategy: draft.restorePathStrategy,
        flattenConflictStrategy: draft.flattenConflictStrategy,
        decryptionPassword: required<HTMLInputElement>(form, "#decryption-password").value,
      });
      setFormBusy(form, false);
      setNotice(repository.path, "restore", {
        tone: "success",
        message: `Restored ${result.fileCount} files (${formatBytes(result.byteCount)}).`,
      });
      updateVisibleNotice(getNotice(repository.path, "restore")!);
    } catch (error) {
      setFormBusy(form, false);
      setNotice(repository.path, "restore", { tone: "error", message: String(error) });
      updateVisibleNotice({ tone: "error", message: String(error) });
    }
  });
}

function bindResizer(): void {
  const setWidth = (value: number): void => {
    state.sidebarWidth = Math.min(380, Math.max(220, value));
    document.documentElement.style.setProperty("--sidebar-width", `${state.sidebarWidth}px`);
    resizer.setAttribute("aria-valuenow", String(state.sidebarWidth));
    persist();
  };
  resizer.setAttribute("aria-valuemin", "220");
  resizer.setAttribute("aria-valuemax", "380");
  resizer.addEventListener("pointerdown", (event) => {
    resizer.setPointerCapture(event.pointerId);
    document.body.classList.add("is-resizing");
  });
  resizer.addEventListener("pointermove", (event) => {
    if (resizer.hasPointerCapture(event.pointerId)) setWidth(event.clientX);
  });
  const finish = (event: PointerEvent): void => {
    if (resizer.hasPointerCapture(event.pointerId)) resizer.releasePointerCapture(event.pointerId);
    document.body.classList.remove("is-resizing");
  };
  resizer.addEventListener("pointerup", finish);
  resizer.addEventListener("pointercancel", finish);
  resizer.addEventListener("keydown", (event) => {
    if (event.key === "ArrowLeft") setWidth(state.sidebarWidth - 10);
    else if (event.key === "ArrowRight") setWidth(state.sidebarWidth + 10);
    else if (event.key === "Home") setWidth(220);
    else if (event.key === "End") setWidth(380);
    else return;
    event.preventDefault();
  });
}

async function validatePersistedRepositories(): Promise<void> {
  await Promise.all(
    state.repositories
      .filter((repository) => !repository.archived)
      .map(async (repository) => {
        try {
          const info = await repositoryApi.open(repository.path);
          repository.name = info.name;
          unavailableRepositories.delete(repository.path);
        } catch {
          unavailableRepositories.add(repository.path);
        }
      }),
  );
}

async function bootstrap(): Promise<void> {
  state = await loadState();
  bindResizer();
  if ("__TAURI_INTERNALS__" in window) await validatePersistedRepositories();
  const active = activeRepository();
  if (active && !active.archived && !unavailableRepositories.has(active.path)) {
    try {
      snapshotsByRepository.set(active.path, await repositoryApi.listSnapshots(active.path));
    } catch {
      unavailableRepositories.add(active.path);
    }
  }
  renderAll();
}

void bootstrap().catch((error) => {
  workspaceRoot.innerHTML = '<div class="empty-workspace"><div><h1>BackupTool failed to start</h1><p id="startup-error"></p></div></div>';
  required<HTMLElement>(workspaceRoot, "#startup-error").textContent = String(error);
});
