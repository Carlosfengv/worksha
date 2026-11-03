import { test, expect } from "@playwright/test";
import { fitsRecoveryBudget } from "../../src/shared/lib/recovery-storage";
import type {} from "./recovery-storage-harness";

test("manual cleanup keeps an oversized record whose unseen suffix changed", async ({
  page,
}) => {
  await page.goto("/tests/frontend/recovery-storage.html");
  const result = await page.evaluate(async () => {
    const { providers } = window.recoveryStorageFixture;
    const key = "local-agent.provider-operation.v1.oversized";
    const original = "x".repeat(2 * 1024 * 1024) + "a";
    localStorage.setItem(key, original);
    const record = providers.providerRecoveryRecords().records[0];
    const replacement = original.slice(0, -1) + "b";
    localStorage.setItem(key, replacement);
    let rejected = false;
    try {
      await providers.discardReviewedProviderRecord(record);
    } catch {
      rejected = true;
    }
    const kept = localStorage.getItem(key) === replacement;
    const refreshed = providers.providerRecoveryRecords().records[0];
    if (refreshed) await providers.discardReviewedProviderRecord(refreshed);
    return {
      truncated: record.truncated,
      rejected,
      kept,
      removedAfterReview: localStorage.getItem(key) === null,
    };
  });
  expect(result).toEqual({
    truncated: true,
    rejected: true,
    kept: true,
    removedAfterReview: true,
  });
});

test("recovery budgets preserve byte and character limits for non-ASCII input", () => {
  expect(fitsRecoveryBudget("密钥", 2, "characters")).toBe(true);
  expect(fitsRecoveryBudget("密钥", 5)).toBe(false);
  expect(fitsRecoveryBudget("密钥", 6)).toBe(true);
  expect(fitsRecoveryBudget("", -1)).toBe(false);
});
test("manual cleanup rechecks storage after asynchronous hashing and keeps records when hashing fails", async ({
  page,
}) => {
  await page.goto("/tests/frontend/recovery-storage.html");
  const outcomes = await page.evaluate(async () => {
    const { providers } = window.recoveryStorageFixture;
    const key = "local-agent.provider-operation.v1.oversized";
    const raw = "x".repeat(2 * 1024 * 1024) + "a";
    const originalDigest = crypto.subtle.digest;
    const outcomes = [];
    try {
      for (const mode of ["changed-during-hash", "digest-unavailable"]) {
        crypto.subtle.digest = originalDigest;
        localStorage.setItem(key, raw);
        const record = providers.providerRecoveryRecords().records[0];
        const replacement = raw.slice(0, -1) + "b";
        crypto.subtle.digest = async function (algorithm, data) {
          if (mode === "digest-unavailable") throw Error("digest unavailable");
          const hash = await originalDigest.call(this, algorithm, data);
          localStorage.setItem(key, replacement);
          return hash;
        };
        let rejected = false;
        try {
          await providers.discardReviewedProviderRecord(record);
        } catch {
          rejected = true;
        }
        outcomes.push({
          rejected,
          retained:
            localStorage.getItem(key) ===
            (mode === "digest-unavailable" ? raw : replacement),
        });
      }
    } finally {
      crypto.subtle.digest = originalDigest;
    }
    return outcomes;
  });
  expect(outcomes).toEqual([
    { rejected: true, retained: true },
    { rejected: true, retained: true },
  ]);
});
test("a write must survive readback and cannot replace an existing reminder", async ({
  page,
}) => {
  await page.goto("/tests/frontend/recovery-storage.html");
  const result = await page.evaluate(() => {
    const { shared } = window.recoveryStorageFixture;
    const key = "fixture-write";
    const original = Storage.prototype.setItem;
    let readbackFailed = false,
      overwriteFailed = false;
    try {
      Storage.prototype.setItem = () => {};
      try {
        shared.writeRecoveryRecord(key, "original", 32);
      } catch {
        readbackFailed = true;
      }
    } finally {
      Storage.prototype.setItem = original;
    }
    shared.writeRecoveryRecord(key, "original", 32);
    try {
      shared.writeRecoveryRecord(key, "replacement", 32);
    } catch {
      overwriteFailed = true;
    }
    return { readbackFailed, overwriteFailed, raw: localStorage.getItem(key) };
  });
  expect(result).toEqual({
    readbackFailed: true,
    overwriteFailed: true,
    raw: "original",
  });
});
test("Provider cleanup rejects another database's request and detects silently failed removal", async ({
  page,
}) => {
  await page.goto("/tests/frontend/recovery-storage.html");
  const result = await page.evaluate(() => {
    const { providers, request } = window.recoveryStorageFixture;
    providers.rememberProviderOperation(request);
    const key = providers.providerRecoveryRecords().records[0].key;
    const changed = JSON.stringify({
      ...request,
      storeId: "6087597b-0625-42ea-8fd0-e51f6ad16d01",
    });
    localStorage.setItem(key, changed);
    let foreignRejected = false,
      removeFailed = false;
    try {
      providers.forgetProviderOperation(request);
    } catch {
      foreignRejected = true;
    }
    const foreignKept = localStorage.getItem(key) === changed;
    localStorage.setItem(key, JSON.stringify(request));
    const original = Storage.prototype.removeItem;
    try {
      Storage.prototype.removeItem = () => {};
      try {
        providers.forgetProviderOperation(request);
      } catch {
        removeFailed = true;
      }
    } finally {
      Storage.prototype.removeItem = original;
    }
    const retained = providers.providerRecoveryRecords().records.length;
    providers.forgetProviderOperation(request);
    return {
      foreignRejected,
      foreignKept,
      removeFailed,
      retained,
      remaining: providers.providerRecoveryRecords().records.length,
    };
  });
  expect(result).toEqual({
    foreignRejected: true,
    foreignKept: true,
    removeFailed: true,
    retained: 1,
    remaining: 0,
  });
});
