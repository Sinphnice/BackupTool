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
    includePathContains: optionalText(filter.includePath),
    excludePathContains: optionalText(filter.excludePath),
    extensions: optionalText(filter.extensions),
    includeNameContains: optionalText(filter.includeName),
    excludeNameContains: optionalText(filter.excludeName),
    minSize: optionalNumber(filter.minSize),
    maxSize: optionalNumber(filter.maxSize),
    modifiedAfter: optionalUnixSeconds(filter.modifiedAfter),
    modifiedBefore: optionalUnixSeconds(filter.modifiedBefore),
  };
}

export const repositoryApi = {
  create(parentPath: string, name: string): Promise<RepositoryInfo> {
    return invoke("create_repository", { parentPath, name });
  },
  open(repositoryPath: string): Promise<RepositoryInfo> {
    return invoke("open_repository", { repositoryPath });
  },
  listSnapshots(repositoryPath: string): Promise<SnapshotInfo[]> {
    return invoke("list_snapshots", { repositoryPath });
  },
  backup(args: {
    repositoryPath: string;
    sources: string[];
    filter: BackupFilterDraft;
    compressionAlgorithm: string;
    encryptionAlgorithm: string;
    encryptionPassword: string;
    snapshotTitle: string;
  }): Promise<BackupResult> {
    return invoke("backup", {
      sources: args.sources,
      destination: args.repositoryPath,
      filter: filterPayload(args.filter),
      compressionAlgorithm: args.compressionAlgorithm,
      encryptionAlgorithm: args.encryptionAlgorithm,
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
  deleteSnapshot(repositoryPath: string, snapshotId: string): Promise<SnapshotDeleteResult> {
    return invoke("delete_snapshot", { repositoryPath, snapshotId });
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

export function confirmSnapshotDeletion(title: string): Promise<boolean> {
  return confirm(`Delete snapshot “${title}”? Unreferenced objects will also be removed.`, {
    title: "Delete snapshot",
    kind: "warning",
  });
}
