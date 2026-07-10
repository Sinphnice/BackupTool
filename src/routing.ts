import type { AppState, RepositoryRecord, WorkspaceRoute } from "./types";
import { ensureWorkspace } from "./state";

export function repositoryByPath(
  state: AppState,
  path: string | undefined,
): RepositoryRecord | undefined {
  return path ? state.repositories.find((repository) => repository.path === path) : undefined;
}

export function activeRepository(state: AppState): RepositoryRecord | undefined {
  return repositoryByPath(state, state.activeRepositoryPath);
}

export function updateWorkspaceRoute(
  state: AppState,
  repositoryPath: string,
  route: WorkspaceRoute,
): void {
  ensureWorkspace(state, repositoryPath).route = route;
}
