import Handlebars from "handlebars";
import type { Task, Validation } from "./model.js";
import { escapeRegex } from "./utils.js";

Handlebars.registerHelper("escapeRegex", escapeRegex);

export type ValidationResult = {
  name: string;
  passed: boolean;
  message: string;
};

function validateRegex(output: string, regex: string, name: string): ValidationResult {
  const pattern = new RegExp(regex);
  const passed = pattern.test(output);
  return { name, passed, message: passed ? `Matched: ${regex}` : `Did not match: ${regex}` };
}

async function validateShellCommand(
  output: string,
  command: string,
  expectedExitCode: number,
  name: string,
): Promise<ValidationResult> {
  try {
    const { spawn } = await import("child_process");
    const result = await new Promise<{ code: number | null; signal: NodeJS.Signals | null }>((resolve, reject) => {
      const child = spawn(command, { shell: true, stdio: ["pipe", "pipe", "pipe"] });
      let processExited = false;
      child.stdin.on("error", (err: NodeJS.ErrnoException) => { if (err.code !== "EPIPE") reject(err); });
      child.on("close", (code, signal) => { processExited = true; resolve({ code, signal }); });
      child.on("error", reject);
      setImmediate(() => {
        if (!processExited && child.stdin.writable) {
          child.stdin.write(output, (err?: Error | null) => {
            if (err && (err as NodeJS.ErrnoException).code !== "EPIPE") reject(err);
            else child.stdin.end();
          });
        } else {
          child.stdin.end();
        }
      });
    });
    const passed = result.signal === null && result.code === expectedExitCode;
    const observed = result.signal !== null ? `signal ${result.signal}` : `exit code ${String(result.code)}`;
    return {
      name,
      passed,
      message: passed ? `Command succeeded with ${observed}` : `Expected exit code ${expectedExitCode}, got ${observed}`,
    };
  } catch (error: any) {
    return { name, passed: false, message: `Command failed: ${error.message}` };
  }
}

export async function runValidations(
  output: string,
  validations: Array<Validation>,
  context?: Record<string, string>,
): Promise<ValidationResult[]> {
  const results: ValidationResult[] = [];
  for (const validation of validations) {
    if (validation.type === "regex") {
      let regex = validation.regex;
      if (context) regex = Handlebars.compile(regex, { strict: true })(context);
      results.push(validateRegex(output, regex, validation.name));
    } else if (validation.type === "shell") {
      let command = validation.command;
      if (context) command = Handlebars.compile(command, { strict: true })(context);
      results.push(await validateShellCommand(output, command, validation.exit_code ?? 0, validation.name));
    } else {
      results.push({ name: validation.name, passed: false, message: `Unsupported validation type: ${validation.type}` });
    }
  }
  return results;
}

export function allValidationsPassed(results: ValidationResult[]): boolean {
  return results.length > 0 && results.every((result) => result.passed);
}

export function countPassed(results: ValidationResult[]): number {
  return results.filter((result) => result.passed).length;
}

export type ProcessValidationsResult = {
  validationResults: ValidationResult[];
  status: "passed" | "validation_failed";
};

export async function processValidations(
  output: string | undefined,
  task: Task,
  logger: {
    info: (data: any, message: string) => void;
    warn: (data: any, message: string) => void;
    error: (data: any, message: string) => void;
  },
  task_id: number,
  duration: number,
  logFile: string,
  context?: Record<string, string>,
): Promise<ProcessValidationsResult> {
  const declared = task.validations ?? [];
  if (declared.length === 0) {
    if (task.validation_policy === "execution_only") {
      logger.info({ task_id, duration, log: logFile, parameters: context }, "Validation policy explicitly accepts command execution only");
      return { validationResults: [], status: "passed" };
    }
    const validationResults: ValidationResult[] = [{
      name: "__validation_policy__",
      passed: false,
      message: "No validations configured. Set validation_policy: execution_only only when command exit status is the complete acceptance policy.",
    }];
    logger.error({ task_id, duration, log_file: logFile, parameters: context, failed: validationResults, summary: "0/1 passed" }, "Validation Failed");
    return { validationResults, status: "validation_failed" };
  }

  const validationResults = await runValidations(output ?? "", declared, context);
  const allPassed = allValidationsPassed(validationResults);
  const status = allPassed ? "passed" : "validation_failed";
  const passedCount = countPassed(validationResults);
  const totalCount = validationResults.length;
  if (allPassed) {
    logger.info({ task_id, duration, log: logFile, parameters: context, passed: validationResults.map((r) => r.name) }, "Validation passed");
  } else {
    logger.error({
      task_id,
      duration,
      log_file: logFile,
      parameters: context,
      failed: validationResults.filter((r) => !r.passed).map((r) => ({ name: r.name, message: r.message })),
      summary: `${passedCount}/${totalCount} passed`,
    }, "Validation Failed");
  }
  return { validationResults, status };
}
