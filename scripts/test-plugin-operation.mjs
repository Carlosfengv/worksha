import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { submitPlatformAction } from "../src/features/plugins/platform-operation.ts";

const create = {
  actionId: "records.create",
  pluginId: "board",
  digest: "package",
  expectedEpoch: "1",
  expectedGrantRevision: "1.0",
  values: { title: "first" },
};
const receipt = { grantRevision: "1.0", recordId: "card-1" };

beforeEach(() => {
  const rows = new Map();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key) => rows.get(key) ?? null,
      setItem: (key, value) => rows.set(key, value),
      removeItem: (key) => rows.delete(key),
    },
  });
});

test("an incomplete write acknowledgement survives reload until the original store confirms the receipt", async () => {
  let operationId;
  await assert.rejects(
    submitPlatformAction(create, "store", async (body) => {
      operationId = body.operationId;
      return { grantRevision: "1.0", recordId: null };
    }),
    /回执/,
  );
  await assert.rejects(
    submitPlatformAction(create, "another-store", () =>
      assert.fail("unexpected write"),
    ),
    /原连接/,
  );
  for (const result of [
    { ...receipt, grantRevision: "2.0" },
    { ...receipt, pending: true },
    receipt,
  ]) {
    const recovery = submitPlatformAction(
      { ...create },
      "store",
      async (body) => {
        assert.equal(body.receiptOnly, true);
        assert.equal(body.operationId, operationId);
        return result;
      },
    );
    if (result === receipt) assert.deepEqual(await recovery, receipt);
    else await assert.rejects(recovery, /回执/);
  }
  await submitPlatformAction(create, "store", async (body) => {
    assert.notEqual(body.operationId, operationId);
    assert.equal(body.receiptOnly, undefined);
    return receipt;
  });
});

test("a host with no receipt reuses the original operation id after transport loss", async () => {
  const ids = [];
  await assert.rejects(
    submitPlatformAction(create, "store", async (body) => {
      ids.push(body.operationId);
      throw Error("reply lost");
    }),
    /reply lost/,
  );
  const result = await submitPlatformAction(create, "store", async (body) => {
    ids.push(body.operationId);
    return body.receiptOnly ? { pending: true } : receipt;
  });
  assert.deepEqual(result, receipt);
  assert.equal(ids.length, 3);
  assert.equal(new Set(ids).size, 1);
});

test("delete accepts an empty record id and an ambiguous legacy write is never replayed", async () => {
  const remove = { ...create, actionId: "records.delete", recordId: "card-1" };
  const deleted = { ...receipt, recordId: null };
  assert.deepEqual(
    await submitPlatformAction(remove, "store", async () => deleted),
    deleted,
  );
  await assert.rejects(
    submitPlatformAction(create, null, async (body) => {
      assert.equal(body.operationId, undefined);
      throw Error("reply lost");
    }),
    /reply lost/,
  );
  await assert.rejects(
    submitPlatformAction(create, null, () =>
      assert.fail("unexpected legacy write"),
    ),
    /核查/,
  );
});
