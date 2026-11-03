import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { providersApi } from "../api";
import { validateCredentialInput } from "../credential-input";
import { DesktopError } from "@/shared/lib/desktop-command";
import { subscribeBackendRecovery } from "@/shared/lib/backend-recovery";
import type { LegacyCredentialPreview } from "../legacy-credential-receipt";
import type {
  CredentialDiscoveryPage,
  CredentialDiscoveryRequest,
  DiscoveredCredential,
} from "../credential-discovery";
import type {
  CredentialInitialization,
  CredentialNamespace,
  RecoverableCredentialReceipt,
  ScopedCredentialOperation,
  CredentialOperationV3,
} from "../credential-receipt";
import {
  CREDENTIAL_RECORD_LIMIT,
  CREDENTIAL_RECORD_PREFIXES,
  readCredentialRecords,
  rememberCredentialRecord,
  forgetCredentialRecord,
  type CredentialRecoveryRecord,
} from "../credential-recovery-records";

function changed(): never {
  throw new DesktopError({
    code: "credential_store_changed",
    outcomeUnknown: true,
    message: "凭据操作仍属于原数据库和恢复空间，已保留记录；未采用迟到结果。",
  });
}
export function useCredentialOperations(
  originStoreId: string | undefined,
  activeStoreId: string | null,
) {
  const [records, setRecords] = useState<CredentialRecoveryRecord[]>(() => {
    try {
      return readCredentialRecords();
    } catch {
      return [];
    }
  });
  const [busy, setBusy] = useState(false),
    [namespace, setNamespace] = useState<CredentialNamespace | null>(null),
    [namespaceError, setNamespaceError] = useState<string | null>(null),
    [loadingNamespace, setLoadingNamespace] = useState(false),
    [contextToken, setContextToken] = useState(0),
    [recoveryEpoch, setRecoveryEpoch] = useState(0);
  const currentRecords = useRef(records),
    unconfirmedCreates = useRef(new Set<string>()),
    mounted = useRef(false),
    generation = useRef(0),
    running = useRef<number | null>(null);
  const sync = useCallback((keepReceipts = true) => {
    const rows = readCredentialRecords().map((row) =>
      keepReceipts
        ? (currentRecords.current.find(
            (saved) =>
              saved.storageKey === row.storageKey &&
              saved.raw === row.raw &&
              saved.rawLength === row.rawLength,
          ) ?? row)
        : row,
    );
    currentRecords.current = rows;
    if (mounted.current) setRecords(rows);
  }, []);
  useLayoutEffect(() => {
    mounted.current = true;
    generation.current++;
    setContextToken((value) => value + 1);
    running.current = null;
    setBusy(false);
    setLoadingNamespace(false);
    setNamespace(null);
    setNamespaceError(null);
    try {
      sync(false);
    } catch {
      /* A mutation must still successfully read storage. */
    }
    return () => {
      mounted.current = false;
      running.current = null;
    };
  }, [activeStoreId, originStoreId, sync]);
  useEffect(() => {
    const refresh = () => {
      generation.current++;
      setContextToken((value) => value + 1);
      running.current = null;
      setBusy(false);
      setNamespace(null);
      setNamespaceError(null);
      setRecoveryEpoch((value) => value + 1);
      try {
        sync(false);
      } catch {
        /* Keep known IDs. */
      }
    };
    const storage = (event: StorageEvent) => {
      if (
        !event.key ||
        CREDENTIAL_RECORD_PREFIXES.some((prefix) =>
          event.key!.startsWith(prefix),
        )
      ) {
        try {
          sync();
        } catch {
          /* Keep known IDs. */
        }
      }
    };
    const unsubscribe = subscribeBackendRecovery(refresh);
    window.addEventListener("storage", storage);
    return () => {
      unsubscribe();
      window.removeEventListener("storage", storage);
    };
  }, [sync]);
  useEffect(() => {
    if (!originStoreId || originStoreId !== activeStoreId) return;
    let active = true;
    const token = generation.current;
    setLoadingNamespace(true);
    void providersApi
      .credentialNamespace(originStoreId)
      .then((result) => {
        if (active && generation.current === token) {
          setNamespace(result);
          setNamespaceError(null);
        }
      })
      .catch((reason) => {
        if (active && generation.current === token)
          setNamespaceError(
            reason instanceof Error ? reason.message : String(reason),
          );
      })
      .finally(() => {
        if (active) setLoadingNamespace(false);
      });
    return () => {
      active = false;
    };
  }, [originStoreId, activeStoreId, recoveryEpoch]);
  const current = (token: number) =>
    mounted.current &&
    generation.current === token &&
    !!originStoreId &&
    originStoreId === activeStoreId;
  const assertCurrent = (token: number) => {
    if (!current(token)) changed();
  };
  const matches = (
    record: CredentialRecoveryRecord,
    protocol: CredentialNamespace | null,
  ) =>
    !record.issue &&
    record.storeId === originStoreId &&
    !!originStoreId &&
    originStoreId === activeStoreId &&
    !!protocol &&
    (record.version === 3
      ? record.namespaceId === protocol.namespaceId &&
        protocol.namespaceState === "active"
      : record.version === 2 && protocol.legacyState !== "unverified");
  const canUse = (record: CredentialRecoveryRecord) =>
    !record.legacySourceDigest && matches(record, namespace);
  const canRecoverLegacy = (record: CredentialRecoveryRecord) =>
    !record.issue &&
    !!originStoreId &&
    originStoreId === activeStoreId &&
    namespace?.namespaceState === "active" &&
    (record.version === 1 ||
      (record.version === 3 &&
        !!record.legacySourceDigest &&
        record.storeId === originStoreId &&
        record.namespaceId === namespace.namespaceId));
  const remember = (
    record: Parameters<typeof rememberCredentialRecord>[0] & {
      receipt?: RecoverableCredentialReceipt;
    },
  ) => {
    const saved = rememberCredentialRecord(record);
    sync();
    currentRecords.current = currentRecords.current.map((row) =>
      row.storageKey === saved.storageKey
        ? { ...saved, receipt: record.receipt }
        : row,
    );
    if (mounted.current) setRecords(currentRecords.current);
    return saved;
  };
  const forget = (record: CredentialRecoveryRecord) => {
    forgetCredentialRecord(record);
    unconfirmedCreates.current.delete(record.storageKey);
    sync();
  };
  const run = async <T>(work: (token: number) => Promise<T>): Promise<T> => {
    if (running.current !== null)
      throw Error("凭据操作仍在处理中，请等待结果。");
    const token = ++generation.current;
    running.current = token;
    setBusy(true);
    try {
      assertCurrent(token);
      return await work(token);
    } finally {
      if (running.current === token) {
        running.current = null;
        if (mounted.current) setBusy(false);
      }
    }
  };
  const readNamespace = async (token: number) => {
    const result = await providersApi.credentialNamespace(originStoreId);
    assertCurrent(token);
    setNamespace(result);
    setNamespaceError(null);
    return result;
  };
  const storeResult = (
    record: CredentialRecoveryRecord,
    result: ScopedCredentialOperation | CredentialOperationV3,
  ) => {
    if (
      result.version !== record.version ||
      result.storeId !== record.storeId ||
      (result.version === 3 && result.namespaceId !== record.namespaceId) ||
      (result.receipt && result.receipt.credentialLabel !== record.label) ||
      (record.cancelledPending && result.status === "pending")
    )
      throw new DesktopError({
        code: "transport_invalid_response",
        outcomeUnknown: true,
        message: "凭据结果与原恢复空间、名称或阶段不对应，未采用。",
      });
    if (result.status === "cancelled" || result.status === "removed")
      forget(record);
    else {
      remember({
        version: record.version as 2 | 3,
        storeId: record.storeId!,
        namespaceId: record.namespaceId,
        operationId: record.operationId,
        label: record.label,
        cancelledPending: result.status === "cancelled_pending",
        receipt: result.receipt ?? undefined,
      });
      if (result.receipt || result.status !== "pending")
        unconfirmedCreates.current.delete(record.storageKey);
    }
    return result;
  };
  const identity = (record: CredentialRecoveryRecord) => ({
    storeId: record.storeId!,
    namespaceId: record.namespaceId!,
    operationId: record.operationId,
  });
  const query = (record: CredentialRecoveryRecord, cancel: boolean) =>
    record.version === 3
      ? cancel
        ? providersApi.cancelCredentialOperationV3(identity(record))
        : providersApi.credentialOperationV3(identity(record))
      : cancel
        ? providersApi.cancelScopedCredentialOperation(
            record.storeId!,
            record.operationId,
          )
        : providersApi.scopedCredentialOperation(
            record.storeId!,
            record.operationId,
          );
  const create = (
    label: string,
    secret: string,
    retry?: CredentialRecoveryRecord,
  ) =>
    run(async (token) => {
      validateCredentialInput(label, secret);
      const protocol = await readNamespace(token),
        rows = readCredentialRecords();
      if (
        !retry &&
        (rows.length >= CREDENTIAL_RECORD_LIMIT ||
          rows.some((row) => row.issue))
      )
        throw Error("请先核查已有凭据记录，再创建新凭据。");
      if (
        !retry &&
        rows.some((row) => {
          const saved =
            currentRecords.current.find(
              (known) =>
                known.storageKey === row.storageKey &&
                known.raw === row.raw &&
                known.rawLength === row.rawLength,
            ) ?? row;
          return (
            matches(saved, protocol) &&
            !saved.receipt &&
            !saved.cancelledPending
          );
        })
      )
        throw Error("原凭据结果尚未核查，请先核查或取消原操作。");
      if (retry && !matches(retry, protocol)) changed();
      if (!retry && protocol.namespaceState !== "active")
        throw new DesktopError({
          code: "credential_namespace_unverified",
          outcomeUnknown: false,
          message: "请先恢复原材料或明确建立新的恢复空间，未发送密钥。",
        });
      const version = retry?.version ?? 3;
      if (version === 1) throw Error("旧记录未绑定原来源，未自动迁移或重试。");
      const operationId = retry?.operationId ?? crypto.randomUUID(),
        namespaceId =
          version === 3
            ? (retry?.namespaceId ?? protocol.namespaceId)
            : undefined;
      if (retry && retry.label !== label.trim())
        throw Error("原凭据名称不同，未覆盖原记录。");
      remember({
        version,
        storeId: originStoreId!,
        namespaceId,
        operationId,
        label: label.trim(),
      });
      const record = readCredentialRecords().find(
        (row) =>
          row.operationId === operationId &&
          row.storeId === originStoreId &&
          row.version === version &&
          row.namespaceId === namespaceId,
      )!;
      unconfirmedCreates.current.add(record.storageKey);
      try {
        const receipt =
          version === 3
            ? await providersApi.createCredentialV3({
                storeId: originStoreId!,
                namespaceId: namespaceId!,
                operationId,
                label: label.trim(),
                secret,
              })
            : await providersApi.createScopedCredential(
                originStoreId!,
                label.trim(),
                secret,
                operationId,
              );
        assertCurrent(token);
        remember({
          version,
          storeId: originStoreId!,
          namespaceId,
          operationId,
          label: receipt.credentialLabel,
          receipt,
        });
        unconfirmedCreates.current.delete(record.storageKey);
        return receipt;
      } catch (reason) {
        if (!current(token)) throw reason;
        if (
          reason instanceof DesktopError &&
          !reason.outcomeUnknown &&
          [
            "invalid_credential",
            "invalid_credential_operation",
            "credential_operation_capacity",
            "credential_protocol_unavailable",
            "credential_store_changed",
            "credential_namespace_changed",
            "credential_namespace_unverified",
          ].includes(reason.code)
        ) {
          if (!retry) forget(record);
          throw reason;
        }
        if (
          reason instanceof DesktopError &&
          [
            "operation_conflict",
            "credential_operation_cancelled",
            "credential_removed",
          ].includes(reason.code)
        )
          throw reason;
        try {
          const result = await query(record, false);
          assertCurrent(token);
          storeResult(record, result);
          if (result.receipt) return result.receipt;
        } catch {
          /* Preserve the original unknown outcome and ID. */
        }
        throw reason;
      }
    });
  const check = (record: CredentialRecoveryRecord, cancel: boolean) =>
    run(async (token) => {
      const protocol = await readNamespace(token);
      if (!matches(record, protocol)) changed();
      const result = await query(record, cancel);
      assertCurrent(token);
      return storeResult(record, result);
    });
  const confirmBound = (reference: string | null) => {
    for (const record of currentRecords.current)
      if (
        canUse(record) &&
        record.receipt &&
        record.receipt.storeId === originStoreId &&
        record.receipt.credentialRef === reference
      ) {
        try {
          forget(record);
        } catch {
          /* Confirmation remains valid. */
        }
      }
  };
  const dismissCancelled = async (record: CredentialRecoveryRecord) => {
    const result = await check(record, true);
    if (["cancelled_pending", "cancelled", "removed"].includes(result.status)) {
      const saved = currentRecords.current.find(
        (row) => row.storageKey === record.storageKey,
      );
      if (saved) forget(saved);
    }
    return result;
  };
  const refreshNamespace = () =>
    run(async (token) => {
      try {
        return await readNamespace(token);
      } catch (reason) {
        if (current(token))
          setNamespaceError(
            reason instanceof Error ? reason.message : String(reason),
          );
        throw reason;
      }
    });
  const initializeNamespace = (request: CredentialInitialization) =>
    run(async (token) => {
      if (request.storeId !== originStoreId) changed();
      const before = await readNamespace(token);
      if (before.namespaceId !== request.namespaceId) changed();
      try {
        const result =
          await providersApi.initializeCredentialNamespace(request);
        assertCurrent(token);
        setNamespace(result);
        setNamespaceError(null);
        return result;
      } catch (reason) {
        if (current(token))
          try {
            const result = await readNamespace(token);
            if (
              result.namespaceId === request.namespaceId &&
              result.namespaceState === "active" &&
              (request.choice !== "restore" ||
                result.legacyState !== "unverified")
            )
              return result;
          } catch {
            /* Keep the original outcome. */
          }
        throw reason;
      }
    });
  const previewLegacy = (record: CredentialRecoveryRecord) =>
    run(async (token) => {
      if (!canRecoverLegacy(record)) changed();
      const protocol = await readNamespace(token);
      const result = await providersApi.previewLegacyCredential({
        storeId: originStoreId!,
        namespaceId: protocol.namespaceId,
        operationId: record.operationId,
      });
      assertCurrent(token);
      if (
        (record.legacySourceDigest &&
          result.sourceDigest !== record.legacySourceDigest) ||
        (result.label !== null && result.label !== record.label)
      )
        throw Error("旧记录名称或原材料与本机提示不符，请保留原记录人工核查。");
      return result;
    });
  const confirmLegacy = (
    record: CredentialRecoveryRecord,
    preview: LegacyCredentialPreview,
  ) =>
    run(async (token) => {
      if (!canRecoverLegacy(record)) changed();
      const protocol = await readNamespace(token);
      if (
        preview.storeId !== originStoreId ||
        preview.namespaceId !== protocol.namespaceId ||
        preview.operationId !== record.operationId
      )
        changed();
      const currentRecord = readCredentialRecords().find(
        (row) => row.storageKey === record.storageKey,
      );
      if (
        !currentRecord ||
        currentRecord.raw !== record.raw ||
        currentRecord.rawLength !== record.rawLength
      )
        throw Error("本机旧记录已经变化，请重新核查。");
      const target = {
        version: 3 as const,
        storeId: preview.storeId,
        namespaceId: preview.namespaceId,
        operationId: preview.operationId,
        label: preview.label ?? record.label,
      };
      const live = readCredentialRecords();
      const existing = live.find(
        (row) =>
          row.version === 3 &&
          row.storeId === target.storeId &&
          row.namespaceId === target.namespaceId &&
          row.operationId === target.operationId,
      );
      if (!existing && live.length >= CREDENTIAL_RECORD_LIMIT)
        throw Error("恢复提示已满，请先核查并整理其他提示，再恢复旧记录。");
      if (
        existing &&
        (existing.issue ||
          existing.label !== target.label ||
          (existing.legacySourceDigest
            ? existing.legacySourceDigest !== preview.sourceDigest
            : preview.stage !== "committed"))
      )
        throw Error("当前空间已有不同的同编号提示，未覆盖原记录。");
      const pendingHint = remember({
        ...target,
        legacySourceDigest: preview.sourceDigest,
      });
      let result: LegacyCredentialPreview;
      try {
        result = await providersApi.confirmLegacyCredential({
          ...target,
          sourceDigest: preview.sourceDigest,
          confirmed: true,
        });
        assertCurrent(token);
      } catch (reason) {
        assertCurrent(token);
        const recovered = await providersApi
          .previewLegacyCredential(target)
          .catch(() => null);
        assertCurrent(token);
        if (
          !recovered ||
          recovered.sourceDigest !== preview.sourceDigest ||
          recovered.stage !== "committed"
        )
          throw reason;
        result = recovered;
      }
      if (localStorage.getItem(pendingHint.storageKey) !== pendingHint.raw)
        throw Error(
          "后台已记录迁移结果，但本机提示已变化；原提示已保留，请重新核查。",
        );
      remember(target);
      const saved = currentRecords.current.find(
        (row) =>
          row.version === 3 &&
          row.storeId === target.storeId &&
          row.namespaceId === target.namespaceId &&
          row.operationId === target.operationId,
      )!;
      try {
        const state = await query(saved, false);
        assertCurrent(token);
        storeResult(saved, state);
      } catch {
        assertCurrent(token);
        // Migration is committed; the target credential may still be unknown.
      }
      if (record.version === 1)
        try {
          forget(record);
        } catch {
          /* Keep both hints; committed migration remains valid. */
        }
      return result;
    });
  const discover = (cursor: string | null) =>
    run(async (token) => {
      const protocol = await readNamespace(token);
      if (protocol.namespaceState !== "active")
        throw Error("请先核查当前后台的恢复空间，再查找记录。");
      const result = await providersApi.discoverCredentials({
        storeId: originStoreId!,
        namespaceId: protocol.namespaceId,
        cursor,
      });
      assertCurrent(token);
      return result;
    });
  const restoreDiscovered = (
    request: CredentialDiscoveryRequest,
    page: CredentialDiscoveryPage,
    item: DiscoveredCredential,
  ) =>
    run(async (token) => {
      if (
        item.issue ||
        !item.digest ||
        request.storeId !== originStoreId ||
        page.storeId !== request.storeId ||
        page.namespaceId !== request.namespaceId
      )
        changed();
      const protocol = await readNamespace(token);
      if (
        protocol.namespaceId !== page.namespaceId ||
        protocol.namespaceState !== "active"
      )
        changed();
      const latest = await providersApi.discoverCredentials({
        storeId: request.storeId,
        namespaceId: request.namespaceId,
        cursor: request.cursor,
      });
      assertCurrent(token);
      const actual = latest.items.find(
        (row) => row.kind === item.kind && row.operationId === item.operationId,
      );
      if (
        latest.snapshot !== page.snapshot ||
        !actual ||
        actual.issue ||
        actual.digest !== item.digest ||
        actual.label !== item.label ||
        actual.phase !== item.phase ||
        actual.legacyStage !== item.legacyStage
      )
        throw new DesktopError({
          code: "credential_discovery_changed",
          outcomeUnknown: false,
          message: "后台记录已变化，请重新查找并核对；未覆盖本机提示。",
        });
      const target = {
        version: item.kind === "scoped_v2" ? (2 as const) : (3 as const),
        storeId: page.storeId,
        namespaceId: item.kind === "scoped_v2" ? undefined : page.namespaceId,
        operationId: item.operationId,
        label: item.label ?? "已取消的凭据记录",
        cancelledPending: item.phase === "cancelled_pending",
        legacySourceDigest:
          item.legacyStage && item.legacyStage !== "committed"
            ? item.digest!
            : undefined,
      };
      const key =
        CREDENTIAL_RECORD_PREFIXES[target.version - 1] +
        target.storeId +
        "." +
        (target.version === 3 ? target.namespaceId + "." : "") +
        target.operationId;
      const rows = readCredentialRecords(),
        existing = rows.find((row) => row.storageKey === key);
      if (existing) {
        if (
          existing.issue ||
          existing.label !== target.label ||
          existing.legacySourceDigest !== target.legacySourceDigest ||
          !!existing.cancelledPending !== target.cancelledPending
        )
          throw Error(
            "本机已有内容或阶段不同的同编号提示，未覆盖；请先核查已有提示。",
          );
        sync();
        return;
      }
      if (
        rows.length >= CREDENTIAL_RECORD_LIMIT ||
        rows.some((row) => row.issue) ||
        localStorage.getItem(key) !== null
      )
        throw Error(
          "本机提示已满或存在无法核查的记录，请先整理已有提示；未覆盖原记录。",
        );
      remember(target);
      assertCurrent(token);
    });
  const unresolvedRecord = (record: CredentialRecoveryRecord) =>
    (matches(record, namespace) &&
      !record.receipt &&
      !record.cancelledPending) ||
    !!record.issue;
  return {
    records,
    busy,
    storeId: originStoreId,
    namespace,
    namespaceError,
    loadingNamespace,
    contextToken,
    canDiscover:
      !!originStoreId &&
      originStoreId === activeStoreId &&
      namespace?.namespaceState === "active",
    canUse,
    canRecoverLegacy,
    unresolved: records.some(unresolvedRecord),
    // An attempted replacement remains this editor's intent across reconnects;
    // a temporarily unread namespace must not silently fall back to its old Key.
    ownUnresolved: records.some(
      (record) =>
        unconfirmedCreates.current.has(record.storageKey) &&
        ((!record.receipt && !record.cancelledPending) || !!record.issue),
    ),
    create,
    check,
    confirmBound,
    dismissCancelled,
    discardLocal: forget,
    refreshNamespace,
    initializeNamespace,
    previewLegacy,
    confirmLegacy,
    discover,
    restoreDiscovered,
  };
}
