import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { providerDraftRecoveryApi, providersApi } from "../api";
import {
  draftGrantHint,
  draftGrantHints,
  removeDraftGrantHint,
  type DraftHint,
  type DraftRecoveryEntry,
} from "../draft-recovery";
import { message } from "../display";
import { useCatalogContext } from "./use-catalog-context";

export type DraftInspection = Awaited<
  ReturnType<typeof providerDraftRecoveryApi.inspect>
>;
export function useDraftRecovery(storeId: string | undefined, open: boolean) {
  const { capture, isCurrent, epoch } = useCatalogContext(storeId);
  const view = useRef(0),
    active = useRef<number | null>(null),
    serial = useRef(0);
  const [entries, setEntries] = useState<DraftRecoveryEntry[] | null>(null);
  const [detail, setDetail] = useState<DraftInspection | null>(null);
  const [hints, setHints] = useState<ReturnType<typeof draftGrantHints> | null>(
    null,
  );
  const [hintError, setHintError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [fresh, setFresh] = useState(false);
  useLayoutEffect(() => {
    view.current++;
    active.current = null;
    setEntries(null);
    setDetail(null);
    setHints(null);
    setHintError(null);
    setError(null);
    setNotice(null);
    setBusy(null);
    setFresh(false);
  }, [storeId, epoch, open]);
  const loadHints = useCallback(() => {
    try {
      setHints(draftGrantHints());
      setHintError(null);
    } catch (reason) {
      setHints(null);
      setHintError(message(reason));
    }
  }, []);
  const run = useCallback(
    async (
      name: string,
      work: (current: () => boolean, storeId: string) => Promise<void>,
    ) => {
      if (active.current !== null || !open) return;
      const source = capture(),
        generation = view.current;
      if (!source) {
        setError("原后台尚未确认，请连接后核查。");
        return;
      }
      const token = ++serial.current;
      active.current = token;
      setBusy(name);
      setError(null);
      setNotice(null);
      const current = () =>
        view.current === generation &&
        isCurrent(source) &&
        active.current === token;
      try {
        await work(current, source.storeId);
      } catch (reason) {
        if (current()) {
          setError(message(reason));
          setFresh(false);
        }
      } finally {
        if (current()) {
          active.current = null;
          setBusy(null);
        }
      }
    },
    [open, capture, isCurrent],
  );
  const read = useCallback(
    () =>
      run("list", async (current, currentStore) => {
        setFresh(false);
        setDetail(null);
        // A damaged or unavailable browser store does not prevent host recovery.
        loadHints();
        const rows = await providerDraftRecoveryApi.list(currentStore);
        if (current()) {
          setEntries(rows);
          setFresh(true);
        }
      }),
    [run, loadHints],
  );
  const inspect = useCallback(
    (entry: DraftRecoveryEntry) =>
      run("inspect", async (current, currentStore) => {
        if (entry.owner.storeId !== currentStore)
          throw Error("请连接此归属原来的后台。");
        const value = await providerDraftRecoveryApi.inspect(entry.owner);
        if (current()) {
          setDetail(value);
          setEntries(
            (rows) =>
              rows?.map((r) =>
                r.owner.ownerId === entry.owner.ownerId ? value.entry : r,
              ) ?? null,
          );
          setFresh(true);
        }
      }),
    [run],
  );
  const cancel = useCallback(
    (inspection: DraftInspection) =>
      run("cancel", async (current, currentStore) => {
        const request = inspection.request;
        if (
          !request ||
          request.storeId !== currentStore ||
          !["pending", "not-found"].includes(inspection.entry.state)
        )
          throw Error("原请求尚不可核查，没有发送取消。");
        // The original command owns cancellation and its durable SQL receipt. No
        // new ID, source choice, submission or probe is created by this view.
        await providersApi.cancelOperation(structuredClone(request));
        if (!current()) return;
        const value = await providerDraftRecoveryApi.inspect(
          inspection.entry.owner,
        );
        if (current()) {
          setDetail(value);
          setEntries(
            (rows) =>
              rows?.map((r) =>
                r.owner.ownerId === value.entry.owner.ownerId ? value.entry : r,
              ) ?? null,
          );
          setFresh(true);
          setNotice(
            value.entry.state === "cancelled"
              ? "原操作已确认取消；可核对后结束授权归属。"
              : "已核查原操作的实际结果；没有重复提交连接。",
          );
        }
      }),
    [run],
  );
  const finish = useCallback(
    (inspection: DraftInspection) =>
      run("finish", async (current, currentStore) => {
        const entry = inspection.entry;
        if (!entry.canFinish || entry.owner.storeId !== currentStore)
          throw Error("请先重新核查此归属。");
        let snapshot: DraftHint | null = null,
          localError: string | null = null;
        try {
          snapshot = draftGrantHint(entry.owner);
        } catch (reason) {
          localError = message(reason);
        }
        const released = await providerDraftRecoveryApi.finish(entry.owner);
        if (!current()) return;
        if (!released) {
          setFresh(false);
          setNotice(
            "仍有相关操作或结果待核查，归属已保留。请重新读取宿主记录。",
          );
          return;
        }
        if (snapshot) {
          if (snapshot.storeId && snapshot.storeId !== currentStore)
            localError = "本机提示属于其他数据库，已保留。";
          else
            try {
              removeDraftGrantHint(snapshot);
            } catch (reason) {
              localError = message(reason);
            }
        }
        setDetail(null);
        setEntries(
          (rows) =>
            rows?.filter((r) => r.owner.ownerId !== entry.owner.ownerId) ??
            null,
        );
        loadHints();
        setNotice(
          localError
            ? `宿主归属已结束；本机提示暂未移除：${localError}`
            : "归属已结束，连接、凭据和原操作回执保持。服务商操作提示仍可在原恢复列表核查。",
        );
      }),
    [run, loadHints],
  );
  const removeHint = useCallback(
    (snapshot: DraftHint) =>
      run("hint", async (current) => {
        removeDraftGrantHint(snapshot);
        if (current()) {
          loadHints();
          setNotice("已移除此条本机授权提示；宿主权限、原操作和其他提示保持。");
        }
      }),
    [run, loadHints],
  );
  return {
    entries,
    detail,
    hints,
    hintError,
    error,
    notice,
    busy,
    fresh,
    read,
    inspect,
    cancel,
    finish,
    removeHint,
    loadHints,
  };
}
