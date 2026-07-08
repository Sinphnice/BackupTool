import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import "./styles.css";

type OperationResult = {
  fileCount: number;
  byteCount: number;
  snapshotId?: string;
  ignoredSources?: string[];
};

type BackupFilter = {
  includePathContains?: string;
  excludePathContains?: string;
  extensions?: string;
  includeNameContains?: string;
  excludeNameContains?: string;
  minSize?: number;
  maxSize?: number;
  modifiedAfter?: number;
  modifiedBefore?: number;
};

type SnapshotInfo = {
  id: string;
  fileCount: number;
  byteCount: number;
  createdUnixSeconds?: number;
};

const result = required<HTMLParagraphElement>("#core-result");
const startBackup = required<HTMLButtonElement>("#start-backup");
const startRestore = required<HTMLButtonElement>("#start-restore");
const addSource = required<HTMLButtonElement>("#add-source");
const sourceList = required<HTMLDivElement>("#source-list");
const openRepository = required<HTMLButtonElement>("#open-repository");
const currentRepository = required<HTMLSpanElement>("#current-repository");
const snapshotList = required<HTMLSelectElement>("#snapshot-list");
const restorePathStrategy = required<HTMLSelectElement>("#restore-path-strategy");
const flattenConflictRow = required<HTMLElement>("#flatten-conflict-row");
const flattenConflictStrategy = required<HTMLSelectElement>("#flatten-conflict-strategy");
const browseButtons = document.querySelectorAll<HTMLButtonElement>(".browse-button");
const sourcePaths: string[] = [];
let currentRepositoryPath = "";

function required<T extends Element>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) {
    throw new Error(`Missing required UI element: ${selector}`);
  }
  return element;
}

function inputValue(selector: string): string {
  return required<HTMLInputElement>(selector).value.trim();
}

function optionalNumber(selector: string): number | undefined {
  const value = inputValue(selector);
  if (!value) {
    return undefined;
  }
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function optionalUnixSeconds(selector: string): number | undefined {
  const value = inputValue(selector);
  if (!value) {
    return undefined;
  }
  // Tauri 会把数字时间戳传给 backup-core；核心库按 Unix 秒比较文件修改时间。
  const timestamp = new Date(value).getTime();
  return Number.isFinite(timestamp) ? Math.floor(timestamp / 1000) : undefined;
}

function optionalText(selector: string): string | undefined {
  const value = inputValue(selector);
  return value || undefined;
}

function collectFilter(): BackupFilter {
  // 空字段不传给 Rust core，使其表示“未启用该筛选条件”，而不是匹配空字符串。
  return {
    includePathContains: optionalText("#include-path"),
    excludePathContains: optionalText("#exclude-path"),
    extensions: optionalText("#extensions"),
    includeNameContains: optionalText("#include-name"),
    excludeNameContains: optionalText("#exclude-name"),
    minSize: optionalNumber("#min-size"),
    maxSize: optionalNumber("#max-size"),
    modifiedAfter: optionalUnixSeconds("#modified-after"),
    modifiedBefore: optionalUnixSeconds("#modified-before"),
  };
}

function formatOperation(name: string, value: OperationResult): string {
  const snapshot = value.snapshotId ? ` Snapshot: ${value.snapshotId}.` : "";
  const ignored =
    value.ignoredSources && value.ignoredSources.length > 0
      ? ` Ignored ${value.ignoredSources.length} duplicate or nested source paths.`
      : "";
  return `${name} succeeded: ${value.fileCount} files, ${value.byteCount} bytes.${snapshot}${ignored}`;
}

function formatSnapshot(value: SnapshotInfo): string {
  const created = value.createdUnixSeconds
    ? new Date(value.createdUnixSeconds * 1000).toLocaleString()
    : "unknown time";
  return `${created} | ${value.fileCount} files | ${value.byteCount} bytes | ${value.id}`;
}

function selectedSnapshotId(): string {
  return snapshotList.value.trim();
}

async function chooseDirectory(): Promise<string | undefined> {
  const selected = await open({
    directory: true,
    multiple: false,
  });

  if (typeof selected !== "string") {
    return undefined;
  }

  return selected;
}

function renderSourceList(): void {
  sourceList.replaceChildren();
  if (sourcePaths.length === 0) {
    const empty = document.createElement("div");
    empty.className = "source-empty";
    empty.textContent = "No source directories added";
    sourceList.append(empty);
    return;
  }

  sourcePaths.forEach((path, index) => {
    const row = document.createElement("div");
    row.className = "source-row";
    row.tabIndex = 0;

    const text = document.createElement("span");
    text.textContent = path;
    text.title = path;

    const remove = document.createElement("button");
    remove.type = "button";
    remove.textContent = "Delete";
    remove.addEventListener("click", () => {
      sourcePaths.splice(index, 1);
      renderSourceList();
    });

    row.append(text, remove);
    sourceList.append(row);
  });
}

async function loadRepositorySnapshots(repositoryPath: string): Promise<void> {
  result.textContent = "Loading snapshots...";
  snapshotList.replaceChildren();

  const snapshots = await invoke<SnapshotInfo[]>("list_snapshots", {
    repositoryPath,
  });

  if (snapshots.length === 0) {
    const option = document.createElement("option");
    option.value = "";
    option.textContent = "No snapshots available";
    snapshotList.append(option);
    result.textContent = "No snapshots found in repository.";
    return;
  }

  for (const snapshot of snapshots) {
    const option = document.createElement("option");
    option.value = snapshot.id;
    option.textContent = formatSnapshot(snapshot);
    snapshotList.append(option);
  }

  snapshotList.selectedIndex = 0;
  result.textContent = `Loaded ${snapshots.length} snapshots.`;
}

function updateRestoreStrategyControls(): void {
  flattenConflictRow.hidden = restorePathStrategy.value !== "flatten";
}

browseButtons.forEach((button) => {
  button.addEventListener("click", async () => {
    const targetId = button.dataset.target;
    if (!targetId) {
      return;
    }

    try {
      const selected = await chooseDirectory();
      if (selected) {
        required<HTMLInputElement>(`#${targetId}`).value = selected;
      }
    } catch (error) {
      result.textContent = String(error);
    }
  });
});

addSource.addEventListener("click", async () => {
  try {
    const selected = await chooseDirectory();
    if (!selected || sourcePaths.includes(selected)) {
      return;
    }
    sourcePaths.push(selected);
    renderSourceList();
  } catch (error) {
    result.textContent = String(error);
  }
});

openRepository.addEventListener("click", async () => {
  try {
    const selected = await chooseDirectory();
    if (!selected) {
      return;
    }
    currentRepositoryPath = selected;
    currentRepository.textContent = selected;
    currentRepository.title = selected;
    await loadRepositorySnapshots(selected);
  } catch (error) {
    result.textContent = String(error);
  }
});

restorePathStrategy.addEventListener("change", updateRestoreStrategyControls);

startBackup.addEventListener("click", async () => {
  result.textContent = "Running backup...";
  if (sourcePaths.length === 0) {
    result.textContent = "Add at least one source directory before backing up.";
    return;
  }

  try {
    const value = await invoke<OperationResult>("backup", {
      sources: sourcePaths,
      destination: inputValue("#backup-destination"),
      filter: collectFilter(),
    });
    result.textContent = formatOperation("Backup", value);
  } catch (error) {
    result.textContent = String(error);
  }
});

startRestore.addEventListener("click", async () => {
  result.textContent = "Running restore...";
  const snapshotId = selectedSnapshotId();
  if (!currentRepositoryPath) {
    result.textContent = "Open a repository before restoring.";
    return;
  }
  if (!snapshotId) {
    result.textContent = "Load a repository and select a snapshot before restoring.";
    return;
  }

  try {
    const value = await invoke<OperationResult>("restore", {
      backupPath: currentRepositoryPath,
      snapshotId,
      destination: inputValue("#restore-destination"),
      pathStrategy: restorePathStrategy.value,
      flattenConflictStrategy: flattenConflictStrategy.value,
    });
    result.textContent = formatOperation("Restore", value);
  } catch (error) {
    result.textContent = String(error);
  }
});

renderSourceList();
updateRestoreStrategyControls();
