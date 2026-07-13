import { invoke } from "@tauri-apps/api/core";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import type {
  ArchiveResult,
  BackupFilterDraft,
  BackupResult,
  RepositoryInfo,
  RestoreResult,
  SnapshotDeleteResult,
  SnapshotInfo,
} from "./types";

function optionalNumber(value: string): number | undefined {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function optionalUnixSeconds(value: string): number | undefined {
  if (!value) return undefined;
  const parsed = new Date(value).getTime();
  return Number.isFinite(parsed) ? Math.floor(parsed / 1000) : undefined;
}

function optionalText(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed || undefined;
}

function filterPayload(filter: BackupFilterDraft): Record<string, unknown> {
  return {
    pathRegex: optionalText(filter.pathRegex),
    owner: optionalText(filter.owner),
    minSize: optionalNumber(filter.minSize),
    maxSize: optionalNumber(filter.maxSize),
    modifiedAfter: optionalUnixSeconds(filter.modifiedAfter),
    modifiedBefore: optionalUnixSeconds(filter.modifiedBefore),
  };
}

export const repositoryApi = {
  create(parentPath: string, name: string, encryptionAlgorithm: string, encryptionPassword: string): Promise<RepositoryInfo> {
    return invoke("create_repository", {
      parentPath,
      name,
      encryptionAlgorithm,
      encryptionPassword,
    });
  },
  open(repositoryPath: string): Promise<RepositoryInfo> {
    return invoke("open_repository", { repositoryPath });
  },
  rename(repositoryPath: string, displayName: string): Promise<RepositoryInfo> {
    return invoke("rename_repository", { repositoryPath, displayName });
  },
  unlock(repositoryPath: string, encryptionPassword: string): Promise<RepositoryInfo> {
    return invoke("unlock_repository", { repositoryPath, encryptionPassword });
  },
  changePassword(repositoryPath: string, oldPassword: string, newPassword: string): Promise<RepositoryInfo> {
    return invoke("change_repository_password", { repositoryPath, oldPassword, newPassword });
  },
  delete(repositoryPath: string, encryptionPassword?: string): Promise<void> {
    return invoke("delete_repository", { repositoryPath, encryptionPassword });
  },
  listSnapshots(repositoryPath: string): Promise<SnapshotInfo[]> {
    return invoke("list_snapshots", { repositoryPath });
  },
  backup(args: {
    repositoryPath: string;
    sources: string[];
    filter: BackupFilterDraft;
    compressionAlgorithm: string;
    encryptSnapshot: boolean;
    encryptionPassword: string;
    snapshotTitle: string;
  }): Promise<BackupResult> {
    return invoke("backup", {
      sources: args.sources,
      destination: args.repositoryPath,
      filter: filterPayload(args.filter),
      compressionAlgorithm: args.compressionAlgorithm,
      encryptSnapshot: args.encryptSnapshot,
      encryptionPassword: args.encryptionPassword,
      snapshotTitle: args.snapshotTitle.trim(),
    });
  },
  restore(args: {
    repositoryPath: string;
    snapshotId: string;
    destination: string;
    pathStrategy: string;
    flattenConflictStrategy: string;
    decryptionPassword: string;
  }): Promise<RestoreResult> {
    return invoke("restore", {
      backupPath: args.repositoryPath,
      snapshotId: args.snapshotId,
      destination: args.destination,
      pathStrategy: args.pathStrategy,
      flattenConflictStrategy: args.flattenConflictStrategy,
      decryptionPassword: args.decryptionPassword,
    });
  },
  export(repositoryPath: string, archivePath: string): Promise<ArchiveResult> {
    return invoke("export_repository", {
      repositoryPath,
      archivePath,
      algorithm: "tar",
    });
  },
  import(archivePath: string, destination: string): Promise<ArchiveResult> {
    return invoke("import_repository", {
      archivePath,
      destination,
      algorithm: "tar",
    });
  },
  deleteSnapshot(repositoryPath: string, snapshotId: string, encryptionPassword?: string): Promise<SnapshotDeleteResult> {
    return invoke("delete_snapshot", { repositoryPath, snapshotId, encryptionPassword });
  },
};

export async function chooseDirectory(title: string): Promise<string | undefined> {
  const selected = await open({ directory: true, multiple: false, title });
  return typeof selected === "string" ? selected : undefined;
}

export async function chooseTarArchive(): Promise<string | undefined> {
  const selected = await open({
    directory: false,
    multiple: false,
    title: "Open repository archive",
    filters: [{ name: "tar archive", extensions: ["tar"] }],
  });
  return typeof selected === "string" ? selected : undefined;
}

export async function chooseExportPath(defaultPath = "repository.tar"): Promise<string | undefined> {
  return (
    (await save({
      defaultPath,
      title: "Export repository",
      filters: [{ name: "tar archive", extensions: ["tar"] }],
    })) ?? undefined
  );
}

export async function confirmSnapshotDeletion(title: string): Promise<boolean> {
  const message = `Delete snapshot "${title}"? Unreferenced objects will also be removed.`;
  try {
    return await confirm(message, {
      title: "Delete snapshot",
      kind: "warning",
    });
  } catch {
    return window.confirm(message);
  }
}
