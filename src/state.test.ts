import { describe, expect, it } from "vitest";
import {
  createState,
  ensureWorkspace,
  reorderRepositories,
  reconcileSnapshotRoute,
  setRepositoryPinned,
  upsertRepository,
  visiblePinnedRepositories,
  visibleUnpinnedRepositories,
} from "./state";

function repository(path: string, name: string) {
  return { path, name, encryptionAlgorithm: "none" as const };
}

function encryptedRepository(path: string, name: string) {
  return { path, name, encryptionAlgorithm: "aes-256-gcm" as const };
}

describe("repository UI state", () => {
  it("deduplicates canonical Windows paths and unarchives reopened repositories", () => {
    const state = createState();
    const first = upsertRepository(state, repository("C:\\Backups\\Repo", "Repo"));
    first.archived = true;

    const reopened = upsertRepository(state, repository("c:\\backups\\repo", "Repo"));

    expect(state.repositories).toHaveLength(1);
    expect(reopened.archived).toBe(false);
    expect(state.activeRepositoryPath).toBe(reopened.path);
  });

  it("updates encryption metadata when reopening an existing repository record", () => {
    const state = createState();
    upsertRepository(state, repository("C:\\Backups\\Repo", "Repo"));

    const reopened = upsertRepository(state, encryptedRepository("c:\\backups\\repo", "Repo"));

    expect(state.repositories).toHaveLength(1);
    expect(reopened.encryptionAlgorithm).toBe("aes-256-gcm");
  });

  it("separates pinned repositories from the regular repository list", () => {
    const state = createState();
    const older = upsertRepository(state, repository("C:\\older", "Older"));
    older.lastOpenedAt = 10;
    const newer = upsertRepository(state, repository("C:\\newer", "Newer"));
    newer.lastOpenedAt = 20;
    setRepositoryPinned(state, older.path, true);

    expect(visiblePinnedRepositories(state).map((item) => item.name)).toEqual(["Older"]);
    expect(visibleUnpinnedRepositories(state).map((item) => item.name)).toEqual(["Newer"]);
  });

  it("keeps explicit repository order after drag reordering", () => {
    const state = createState();
    const first = upsertRepository(state, repository("C:\\first", "First"));
    const second = upsertRepository(state, repository("C:\\second", "Second"));
    const third = upsertRepository(state, repository("C:\\third", "Third"));

    reorderRepositories(state, false, [third.path, first.path, second.path]);

    expect(visibleUnpinnedRepositories(state).map((item) => item.name)).toEqual([
      "Third",
      "First",
      "Second",
    ]);
  });

  it("keeps independent non-sensitive drafts for each repository", () => {
    const state = createState();
    const first = ensureWorkspace(state, "C:\\first");
    const second = ensureWorkspace(state, "C:\\second");
    first.sourcePaths.push("C:\\source-a");
    first.restoreDestination = "C:\\restore-a";

    expect(second.sourcePaths).toEqual([]);
    expect(second.restoreDestination).toBe("");
    expect(JSON.stringify(state)).not.toContain("password");
  });

  it("returns to overview when a selected snapshot no longer exists", () => {
    const workspace = ensureWorkspace(createState(), "C:\\repo");
    workspace.route = { kind: "restore", snapshotId: "missing" };

    expect(reconcileSnapshotRoute(workspace, new Set(["remaining"]))).toBe(true);
    expect(workspace.route).toEqual({ kind: "overview" });
  });
});
