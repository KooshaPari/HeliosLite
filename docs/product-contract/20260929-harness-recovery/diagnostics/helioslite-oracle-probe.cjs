'use strict';
// Diagnostic decision-flow reproduction, NOT a product acceptance test.
// Transcribed from KooshaPari/HeliosLite@536a25cac1dc21ac97bbc86c7e9af74bd5932780.
// Sources: benchmarks/{verification,task-executor,cli}.ts. No native harness is run.
const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const crypto = require('node:crypto');

function allValidationsPassed(results) {
  return results.every((result) => result.passed);
}
function validationStatus(output, validations) {
  // Restricted to the source's regex branch; no context/shell interpolation.
  const results = validations && validations.length > 0 && output
    ? validations.map((v) => ({ passed: new RegExp(v.regex).test(output) })) : [];
  return allValidationsPassed(results) ? 'passed' : 'validation_failed';
}
function taskStatus(executionResult, validations) {
  if (executionResult.error) return executionResult.isTimeout ? 'timeout' : 'failed';
  return validationStatus(executionResult.output || '', validations);
}
function cliExit(statuses) {
  const failCount = statuses.filter((s) => s === 'failed').length;
  return failCount > 0 ? 1 : 0;
}
(async () => {
  const cases = [
    { id: 'H-POS', observed: validationStatus('OK', [{ regex: '^OK$' }]), predicted: 'passed', acceptable: true },
    { id: 'H-NEG', observed: validationStatus('BAD', [{ regex: '^OK$' }]), predicted: 'validation_failed', acceptable: true },
    { id: 'H-EMPTY-CHECKS', observed: validationStatus('anything', []), predicted: 'passed', acceptable: false },
    { id: 'H-MISSING-OUTPUT', observed: validationStatus('', [{ regex: '^OK$' }]), predicted: 'passed', acceptable: false },
    { id: 'H-TIMEOUT', observed: taskStatus({ output: '', isTimeout: true }, [{ regex: '^OK$' }]), predicted: 'passed', acceptable: false },
    { id: 'H-CLI-EXIT', observed: cliExit(['validation_failed']), predicted: 0, acceptable: false },
  ];
  const termination = await new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ['-e', 'process.kill(process.pid,"SIGTERM")']);
    child.once('error', reject);
    child.once('close', (code, signal) => resolve({ code, signal, coerced: code ?? 0 }));
  });
  assert.equal(termination.signal, 'SIGTERM');
  cases.push({ id: 'H-SIGNAL-AS-ZERO', observed: termination.coerced, predicted: 0, acceptable: false });
  for (const c of cases) assert.equal(c.observed, c.predicted, c.id);
  const receipt = {
    schema: 'harness-recovery-diagnostic/v1', product: 'KooshaPari/HeliosLite',
    source_revision: '536a25cac1dc21ac97bbc86c7e9af74bd5932780',
    source_blobs: { verification: '749619083402134b20582b0d08c9d0c583caeb84', task_executor: 'd1d076a02264a5ef746b768cb2f59c2a757b3cf0', cli: '3f681ce9adcefe1af05335e7173356e5947dbfc5' },
    scope: 'decision-flow transcription plus Node child-process termination; not native product execution',
    script_sha256: crypto.createHash('sha256').update(fs.readFileSync(__filename)).digest('hex'),
    runtime: process.version, platform: process.platform, timestamp: new Date().toISOString(),
    diagnostic_assertions: 'passed', product_acceptance: 'BLOCKED', native_product_tests: 'NOT_RUN',
    cases, termination,
  };
  console.log(JSON.stringify(receipt, null, 2));
})().catch((e) => { console.error(e); process.exitCode = 1; });
