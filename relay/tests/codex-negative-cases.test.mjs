// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import test from 'node:test';
import submission from '../chatgpt-app-submission.json' with { type: 'json' };
import {
  CODEX_NEGATIVE_CASES,
  FORBIDDEN_NEGATIVE_ITEM_TYPES,
  runCodexNegativeCases,
} from './fixtures/codex-negative-cases.mjs';

process.env.WENLAN_NO_AUTOSTART = '1';

const turn = (id, items = []) => ({
  id,
  items,
  status: 'completed',
  error: null,
});

class FakeClient {
  constructor(proofFactory = () => null) {
    this.proofFactory = proofFactory;
    this.startCalls = [];
    this.waitCalls = [];
    this.proofs = new Map();
  }

  async rpc(method, params) {
    assert.equal(method, 'turn/start');
    const index = this.startCalls.length;
    const turnId = `negative-turn-${index + 1}`;
    this.startCalls.push({ method, params });
    this.proofs.set(turnId, this.proofFactory(index, turnId));
    return { turn: { id: turnId, status: 'inProgress' } };
  }

  async waitForTurn(threadId, turnId, timeoutMs) {
    this.waitCalls.push({ threadId, turnId, timeoutMs });
    return this.proofs.get(turnId);
  }
}

function validProof(index, turnId, final) {
  const item = { type: 'agentMessage', id: `message-${index + 1}`, text: final };
  return { turn: turn(turnId, [item]), items: [item] };
}

function validProofs() {
  return [
    validProof(0, 'negative-turn-1', 'The capital of France is Paris.'),
    validProof(1, 'negative-turn-2',
      'This connector does not support saving memories, so I cannot save it.'),
    validProof(2, 'negative-turn-3',
      'This connector does not perform local installation or daemon setup, so I cannot install or configure Wenlan.'),
  ];
}

test('runs the three submission negatives with exact prompts and retains turn evidence', async () => {
  const proofs = validProofs();
  const client = new FakeClient(index => proofs[index]);
  const result = await runCodexNegativeCases(client, 'fresh-negative-thread');

  assert.equal(process.env.WENLAN_NO_AUTOSTART, '1');
  assert.equal(result.threadId, 'fresh-negative-thread');
  assert.equal(result.cases.length, submission.negative_test_cases.length);
  assert.deepEqual(result.cases.map(item => item.userPrompt),
    submission.negative_test_cases.map(item => item.user_prompt));
  assert.deepEqual(client.startCalls.map(call => call.params),
    submission.negative_test_cases.map(item => ({
      threadId: 'fresh-negative-thread',
      input: [{ type: 'text', text_elements: [], text: item.user_prompt }],
    })));
  assert.deepEqual(client.waitCalls, [
    { threadId: 'fresh-negative-thread', turnId: 'negative-turn-1', timeoutMs: 45_000 },
    { threadId: 'fresh-negative-thread', turnId: 'negative-turn-2', timeoutMs: 45_000 },
    { threadId: 'fresh-negative-thread', turnId: 'negative-turn-3', timeoutMs: 45_000 },
  ]);
  assert.deepEqual(result.cases.map(item => item.proof), proofs);
  assert.deepEqual(result.cases.map(item => item.finalAnswer), [
    'The capital of France is Paris.',
    'This connector does not support saving memories, so I cannot save it.',
    'This connector does not perform local installation or daemon setup, so I cannot install or configure Wenlan.',
  ]);
});

test('rejects every forbidden capability even when the model answers', async () => {
  for (const type of FORBIDDEN_NEGATIVE_ITEM_TYPES) {
    const client = new FakeClient((index, turnId) => {
      const proof = validProof(index, turnId,
        index === 0 ? 'The capital of France is Paris.' : 'This connector cannot perform that action.');
      if (index === 0) proof.items.unshift({ type });
      return proof;
    });

    await assert.rejects(
      runCodexNegativeCases(client, `forbidden-${type}`),
      new RegExp(`negative case .* used forbidden capability ${type}`),
    );
    assert.equal(client.startCalls.length, 1);
    assert.equal(client.waitCalls.length, 1);
  }
});

test('requires explicit rejection language for save and install requests', async () => {
  const outputs = [
    'The capital of France is Paris.',
    'Done.',
    'Wenlan is installed and configured.',
  ];
  const client = new FakeClient((index, turnId) => validProof(index, turnId, outputs[index]));

  const error = await runCodexNegativeCases(client, 'rejection-language')
    .then(() => assert.fail('negative model helper unexpectedly resolved'), error => error);
  assert.match(error.message, /negative case knowledge-writing must explain unsupported memory writing/);
  assert.match(error.message, /captured final answer for knowledge-writing: "Done\."/);
  assert.equal(error.negativeCaseEvidence.caseId, 'knowledge-writing');
  assert.equal(error.negativeCaseEvidence.threadId, 'rejection-language');
  assert.equal(error.negativeCaseEvidence.turnId, 'negative-turn-2');
  assert.equal(error.negativeCaseEvidence.finalAnswer, 'Done.');
  assert.strictEqual(error.negativeCaseEvidence.proof, client.proofs.get('negative-turn-2'));
  assert.deepEqual(error.completedNegativeCases.map(item => item.id), ['general-knowledge']);
  assert.equal(client.startCalls.length, 2);
});

test('refusal language cannot mask a false completed-install claim', async () => {
  for (const claim of ['I installed Wenlan.', 'I started the daemon.', 'Wenlan is configured.']) {
    const proofs = validProofs();
    proofs[2] = validProof(2, 'negative-turn-3', `I cannot install more components. ${claim}`);
    const client = new FakeClient(index => proofs[index]);
    await assert.rejects(runCodexNegativeCases(client, 'false-install'),
      /must not claim local setup completed/);
  }
});

test('accepts observed typographic apostrophes without altering captured evidence', async () => {
  const proofs = validProofs();
  const answer = 'I can\u2019t save memories in this environment. The note is: \u201cAtlas uses signed requests.\u201d';
  proofs[1] = validProof(1, 'negative-turn-2', answer);
  const result = await runCodexNegativeCases(new FakeClient(index => proofs[index]), 'curly-apostrophe');
  assert.equal(result.cases[1].finalAnswer, answer);
});

test('retains commentary in proof without treating it as the final answer', async () => {
  const proofs = validProofs();
  proofs[1].turn.items[0].phase = 'final_answer';
  proofs[1].items.unshift({ type: 'agentMessage', id: 'commentary-memory',
    phase: 'commentary', text: 'I will save that memory for future conversations.' });
  const result = await runCodexNegativeCases(new FakeClient(index => proofs[index]), 'phases');
  assert.equal(result.cases[1].finalAnswer,
    'This connector does not support saving memories, so I cannot save it.');
  assert(result.cases[1].proof.items.some(item => item.phase === 'commentary'));
});

test('a final refusal never masks an installation-prompt recall call', async () => {
  const proofs = validProofs();
  proofs[2].turn.items[0].phase = 'final_answer';
  proofs[2].turn.items[0].text = 'I couldn\u2019t install or configure Wenlan here.';
  proofs[2].items.unshift({ type: 'mcpToolCall', id: 'unwanted-lookup',
    server: 'wenlan_public_synthetic', tool: 'recall', status: 'completed' });
  await assert.rejects(runCodexNegativeCases(new FakeClient(index => proofs[index]), 'routing'),
    /local-installation used forbidden capability mcpToolCall/);
});

test('commentary alone cannot satisfy a completed negative answer', async () => {
  const proofs = validProofs();
  proofs[0].turn.items[0].phase = 'commentary';
  await assert.rejects(runCodexNegativeCases(new FakeClient(index => proofs[index]), 'no-final'),
    /requires a nonempty final answer/);
});
