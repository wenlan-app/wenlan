// SPDX-License-Identifier: Apache-2.0
// Bounded model-routing proof for the negative cases in the submission fixture.
import assert from 'node:assert/strict';
import submission from '../../chatgpt-app-submission.json' with { type: 'json' };

const MAX_NEGATIVE_CASES = 3;
export const NEGATIVE_TURN_TIMEOUT_MS = 45_000;

export const FORBIDDEN_NEGATIVE_ITEM_TYPES = Object.freeze([
  'mcpToolCall',
  'commandExecution',
  'fileChange',
  'webSearch',
  'dynamicToolCall',
]);

const expectedCases = submission.negative_test_cases;
assert(Array.isArray(expectedCases), 'submission negative test cases must be an array');
assert.equal(expectedCases.length, MAX_NEGATIVE_CASES,
  'submission negative test case count changed; update the bounded model lane');
assert(expectedCases.every(item => item.tools_triggered === null),
  'submission negative cases must remain tool-free');

export const CODEX_NEGATIVE_CASES = Object.freeze(expectedCases.map((item, index) => Object.freeze({
  id: ['general-knowledge', 'knowledge-writing', 'local-installation'][index],
  description: item.description,
  userPrompt: item.user_prompt,
  expectedOutput: item.expected_output,
  toolsTriggered: item.tools_triggered,
})));

const refusalLanguage = /\b(?:cannot|can't|couldn't|can not|unable|unsupported|not supported|does not support|doesn't support|not able)\b/i;
const affirmativeAction = action => new RegExp(
  `\\b(?:I|we|Wenlan|the connector|the daemon|the connection|the memory|it)` +
  `\\s+(?:(?:have|has|had|did|was|were|is|are|been)\\s+)?` +
  `(?!not\\b)(?:successfully\\s+)?${action}\\b`, 'i',
);

function allItems(proof) {
  const items = [
    ...(Array.isArray(proof?.turn?.items) ? proof.turn.items : []),
    ...(Array.isArray(proof?.items) ? proof.items : []),
  ];
  const seen = new Set();
  return items.filter(item => {
    const key = item?.id ? `${item.type ?? ''}:${item.id}` : item;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function textValue(value) {
  if (typeof value === 'string') return value;
  if (Array.isArray(value)) return value.map(textValue).join('');
  if (value && typeof value === 'object') return textValue(value.text ?? value.content ?? '');
  return '';
}

function finalAnswer(proof) {
  const messages = allItems(proof).filter(item => item?.type === 'agentMessage');
  const finals = messages.filter(item => item.phase === 'final_answer');
  return (finals.length ? finals : messages.filter(item => item.phase == null))
    .map(item => textValue(item.text ?? item.content))
    .filter(Boolean)
    .join('\n')
    .trim();
}

function assertExpectedAnswer(caseId, answer) {
  answer = answer.replace(/[\u2018\u2019]/g, "'");
  assert(answer.length > 0, `negative case ${caseId} requires a nonempty final answer`);
  if (caseId === 'general-knowledge') {
    assert.match(answer, /\bParis\b/i,
      'general-knowledge negative case must answer the general-knowledge question');
    return;
  }

  if (caseId === 'knowledge-writing') {
    assert.match(answer, refusalLanguage,
      `negative case ${caseId} must explain unsupported memory writing`);
    assert.match(answer, /\b(?:save|saving|memory|memories)\b/i,
      'knowledge-writing negative case must address memory saving');
    assert.doesNotMatch(answer, affirmativeAction('(?:save|store|create|saved|stored|created)'),
      'knowledge-writing negative case must not claim a memory was saved');
  } else {
    assert.match(answer, refusalLanguage,
      `negative case ${caseId} must explain unsupported local setup`);
    assert.match(answer, /\b(?:install|installation|daemon|configure|configuration|setup)\b/i,
      'local-installation negative case must address local setup');
    assert.doesNotMatch(answer, affirmativeAction('(?:install|start|configure|installed|started|configured)'),
      'local-installation negative case must not claim local setup completed');
  }
}

function attachFailureEvidence(error, scenario, threadId, turnId, answer, proof, cases) {
  error.negativeCaseEvidence = {
    caseId: scenario.id,
    threadId,
    turnId,
    finalAnswer: answer,
    proof,
  };
  error.completedNegativeCases = cases;
  error.message = `${error.message}; captured final answer for ${scenario.id}: ${JSON.stringify(answer)}`;
  return error;
}

export async function runCodexNegativeCases(client, threadId, {
  timeoutMs = NEGATIVE_TURN_TIMEOUT_MS,
} = {}) {
  assert(client && typeof client.rpc === 'function' && typeof client.waitForTurn === 'function',
    'Codex client with rpc and waitForTurn is required');
  assert(typeof threadId === 'string' && threadId.length > 0, 'Codex thread id is required');
  assert(Number.isFinite(timeoutMs) && timeoutMs >= 0 && timeoutMs <= 90_000,
    'negative model turn timeout must be between 0 and 90000 milliseconds');

  const cases = [];
  for (const scenario of CODEX_NEGATIVE_CASES) {
    const started = await client.rpc('turn/start', {
      threadId,
      input: [{ type: 'text', text_elements: [], text: scenario.userPrompt }],
    });
    const turnId = started?.turn?.id;
    assert(typeof turnId === 'string' && turnId.length > 0,
      `negative case ${scenario.id} requires a turn id`);
    const proof = await client.waitForTurn(threadId, turnId, timeoutMs);
    const answer = finalAnswer(proof);
    try {
      assert.equal(proof?.turn?.id, turnId, `negative case ${scenario.id} returned the wrong turn`);
      assert.equal(proof?.turn?.status, 'completed', `negative case ${scenario.id} must complete`);
      assert.equal(proof?.turn?.error, null, `negative case ${scenario.id} must not have a turn error`);

      const forbidden = allItems(proof).find(item =>
        FORBIDDEN_NEGATIVE_ITEM_TYPES.includes(item?.type));
      assert(!forbidden,
        `negative case ${scenario.id} used forbidden capability ${forbidden?.type ?? 'unknown'}`);
      assertExpectedAnswer(scenario.id, answer);
    } catch (error) {
      throw attachFailureEvidence(error, scenario, threadId, turnId, answer, proof, cases);
    }
    cases.push({ ...scenario, turnId, finalAnswer: answer, proof });
  }

  return { threadId, cases };
}
