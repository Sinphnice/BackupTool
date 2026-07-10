import { describe, expect, it } from "vitest";
import {
  createState,
  ensureWorkspace,
  reconcileSnapshotRoute,
  upsertRepository,
  visibleRepositories,
} from "./state";

describe("repository UI state", () => {
  it("deduplicates canonical Windows paths and unarchives reopened repositories", () => {
    const state = createState();
    const first = upsertRepository(state, { path: "C:\\Backups\\Repo", name: "Repo" });
    first.archived = true;

    const reopened = upsertRepository(state, { path: "c:\\backups\\repo", name: "Repo" });

    expect(state.repositories).toHaveLength(1);
    expect(reopened.archived).toBe(false);
    expect(state.activeRepositoryPath).toBe(reopened.path);
  });

  it("sorts pinned repositories before recently opened repositories", () => {
    const state = createState();
    const older = upsertRepository(state, { path: "C:\\older", name: "Older" });
    older.lastOpenedAt = 10;
    const newer = upsertRepository(state, { path: "C:\\newer", name: "Newer" });
    newer.lastOpenedAt = 20;
    older.pinned = true;

    expect(visibleRepositories(state).map((item) => item.name)).toEqual(["Older", "Newer"]);
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
