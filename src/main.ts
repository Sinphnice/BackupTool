import { invoke } from "@tauri-apps/api/core";
import "./styles.css";

type OperationResult = {
  fileCount: number;
  byteCount: number;
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

const result = required<HTMLParagraphElement>("#core-result");
const startBackup = required<HTMLButtonElement>("#start-backup");
const startRestore = required<HTMLButtonElement>("#start-restore");

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
  return `${name} succeeded: ${value.fileCount} files, ${value.byteCount} bytes.`;
}

startBackup.addEventListener("click", async () => {
  result.textContent = "Running backup...";

  try {
    const value = await invoke<OperationResult>("backup", {
      source: inputValue("#backup-source"),
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

  try {
    const value = await invoke<OperationResult>("restore", {
      backupPath: inputValue("#restore-backup"),
      destination: inputValue("#restore-destination"),
    });
    result.textContent = formatOperation("Restore", value);
  } catch (error) {
    result.textContent = String(error);
  }
});
