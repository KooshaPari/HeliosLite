import { TaskStatus } from "./model.js";

export type ResultLike = { status: TaskStatus };

export function strictExitCode(results: ResultLike[]): number {
  return results.length > 0 &&
    results.every((result) => result.status === TaskStatus.Passed)
    ? 0
    : 1;
}
