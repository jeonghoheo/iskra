#!/usr/bin/env node

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const fixtureUrl = new URL("./fixture.json", import.meta.url);
const fixture = JSON.parse(readFileSync(fileURLToPath(fixtureUrl), "utf8"));
const events = fixture.events;
const checks = [];

function check(name, fn) {
  try {
    fn();
    checks.push({ name, state: "PASS" });
  } catch (error) {
    checks.push({ name, state: "FAIL", detail: error.message });
  }
}

function select(type, predicate = () => true) {
  return events.filter((event) => event.type === type && predicate(event));
}

function restartSequence() {
  return select("process_started")[0]?.seq ?? Number.POSITIVE_INFINITY;
}

check("fixture is explicitly synthetic and contains no answer payload", () => {
  assert.equal(fixture.classification, "synthetic-not-a-customer-incident");
  const forbidden = ["answer", "prompt", "api_key", "token", "secret"];
  for (const event of events) {
    for (const key of forbidden) assert.equal(key in event, false, `forbidden field: ${key}`);
  }
});

check("event sequence is strictly increasing", () => {
  assert.ok(events.length > 0);
  for (let index = 1; index < events.length; index += 1) {
    assert.ok(events[index].seq > events[index - 1].seq);
  }
});

check("one Run and one original Delivery own the incident", () => {
  assert.equal(select("run_completed", (event) => event.run_id === 22).length, 1);
  const created = select("delivery_created", (event) => event.delivery_id === 6);
  assert.equal(created.length, 1);
  assert.equal(created[0].run_id, 22);
  assert.equal(created[0].chunks, 2);
});

check("the crash leaves one sent chunk and one pending chunk", () => {
  const crash = select("process_killed")[0];
  assert.ok(crash);
  const before = events.filter((event) => event.seq < crash.seq && event.delivery_id === 6);
  assert.equal(before.filter((event) => event.type === "chunk_settled" && event.state === "sent").length, 1);
  assert.equal(before.filter((event) => event.type === "chunk_attempt_started" && event.state === "pending").length, 2);
});

check("restart converts only the unresolved chunk to unknown", () => {
  const unknown = select("chunk_marked_unknown", (event) => event.delivery_id === 6);
  assert.equal(unknown.length, 1);
  assert.equal(unknown[0].chunk, 1);
  assert.equal(unknown[0].from, "pending");
  assert.equal(unknown[0].to, "unknown");
  assert.ok(unknown[0].seq > restartSequence());
});

check("restart performs zero automatic model calls or original-delivery retries", () => {
  const recovery = select("recovery_completed");
  assert.equal(recovery.length, 1);
  assert.equal(recovery[0].automatic_delivery_attempts, 0);
  assert.equal(recovery[0].automatic_model_calls, 0);
  const retries = events.filter(
    (event) => event.seq > restartSequence()
      && event.delivery_id === 6
      && event.type === "chunk_attempt_started",
  );
  assert.equal(retries.length, 0);
});

check("only an explicit /result creates the new two-chunk delivery", () => {
  assert.equal(select("result_requested", (event) => event.run_id === 22).length, 1);
  const created = select("delivery_created", (event) => event.delivery_id === 7);
  assert.equal(created.length, 1);
  assert.equal(created[0].run_id, 22);
  assert.equal(created[0].explicit, true);
  assert.equal(created[0].chunks, 2);
  assert.notEqual(created[0].delivery_id, 6);
});

check("the original unknown stays unresolved and message ids are unique", () => {
  const originalResolution = events.filter(
    (event) => event.delivery_id === 6
      && event.chunk === 1
      && event.seq > restartSequence()
      && ["chunk_settled", "chunk_reconciled"].includes(event.type),
  );
  assert.equal(originalResolution.length, 0);

  const messageIds = select("chunk_settled", (event) => event.state === "sent")
    .map((event) => event.message_id);
  assert.equal(messageIds.length, 3);
  assert.equal(new Set(messageIds).size, messageIds.length);
  assert.deepEqual(
    select("chunk_settled", (event) => event.delivery_id === 7).map((event) => event.message_id),
    [397, 398],
  );
});

const failed = checks.filter((entry) => entry.state === "FAIL");
const restart = restartSequence();
const hasRestart = Number.isFinite(restart);
const recovery = select("recovery_completed")[0];
const retriesAfterRestart = hasRestart
  ? events.filter(
    (event) => event.seq > restart
      && event.delivery_id === 6
      && event.type === "chunk_attempt_started",
  )
  : null;
const originalUnknown = select("chunk_marked_unknown", (event) => event.delivery_id === 6 && event.chunk === 1)[0];
const originalResolution = originalUnknown && hasRestart
  ? events.filter(
    (event) => event.delivery_id === 6
      && event.chunk === 1
      && event.seq > restart
      && ["chunk_settled", "chunk_reconciled"].includes(event.type),
  )
  : null;
const report = {
  incident: fixture.incident.id,
  schema: fixture.schema,
  state: failed.length === 0 ? "PASS" : "FAIL",
  passed: checks.length - failed.length,
  failed: failed.length,
  automatic_delivery_attempts_after_restart: retriesAfterRestart?.length ?? null,
  automatic_model_calls_after_restart: hasRestart ? recovery?.automatic_model_calls ?? null : null,
  original_unknown_unresolved: originalResolution ? originalResolution.length === 0 : null,
  checks,
};

console.log(JSON.stringify(report, null, 2));
if (failed.length > 0) process.exitCode = 1;
