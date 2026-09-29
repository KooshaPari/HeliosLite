import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { allValidationsPassed, processValidations, runValidations } from "./verification.js";
import { executeTask } from "./task-executor.js";
import { TaskStatus, type Task } from "./model.js";
import { strictExitCode } from "./result-policy.js";

const logger = { info() {}, warn() {}, error() {} };
function task(overrides: Partial<Task> = {}): Task {
  return { before_run: [], run: "true", sources: [{ value: [{}] }], ...overrides };
}

test("empty validation result is never green", () => {
  assert.equal(allValidationsPassed([]), false);
});

test("configured regex runs against empty output instead of being skipped", async () => {
  const result = await processValidations("", task({
    validations: [{ name: "must emit OK", type: "regex", regex: "^OK$" }],
  }), logger, 1, 0, "test.log");
  assert.equal(result.status, "validation_failed");
  assert.equal(result.validationResults[0]?.passed, false);
});

test("validator-free task requires explicit execution-only policy", async () => {
  const result = await processValidations("", task(), logger, 1, 0, "test.log");
  assert.equal(result.status, "validation_failed");
  assert.equal(result.validationResults[0]?.name, "__validation_policy__");
});

test("execution-only policy is explicit and narrow", async () => {
  const result = await processValidations("", task({ validation_policy: "execution_only" }), logger, 1, 0, "test.log");
  assert.equal(result.status, "passed");
  assert.deepEqual(result.validationResults, []);
});

test("unsupported llm validation fails closed", async () => {
  const results = await runValidations("", [{ name: "not implemented", type: "llm", evaluator: "fixture", criteria: {} }]);
  assert.equal(results[0]?.passed, false);
  assert.match(results[0]?.message ?? "", /Unsupported validation type/);
});

test("signalled shell verifier cannot masquerade as exit zero", async () => {
  if (process.platform === "win32") return;
  const command = "kill -TERM $";
  const results = await runValidations("ignored", [{ name: "signal", type: "shell", command, exit_code: 0 }]);
  assert.equal(results[0]?.passed, false);
  assert.match(results[0]?.message ?? "", /signal SIGTERM/);
});

test("timeout is a terminal execution failure", async () => {
  const dir = await mkdtemp(join(tmpdir(), "forge-eval-timeout-"));
  try {
    const command = `${JSON.stringify(process.execPath)} -e "setTimeout(() => {}, 5000)"`;
    const result = await executeTask(command, 1, join(dir, "task.log"), dir, task({ timeout: 0.05 }));
    assert.equal(result.isTimeout, true);
    assert.match(result.error ?? "", /timed out/);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("strict CLI exit policy rejects every non-pass terminal state", () => {
  assert.equal(strictExitCode([{ status: TaskStatus.Passed }]), 0);
  assert.equal(strictExitCode([{ status: TaskStatus.ValidationFailed }]), 1);
  assert.equal(strictExitCode([{ status: TaskStatus.Timeout }]), 1);
  assert.equal(strictExitCode([{ status: TaskStatus.Failed }]), 1);
  assert.equal(strictExitCode([]), 1);
});
