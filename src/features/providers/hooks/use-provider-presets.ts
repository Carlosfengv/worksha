import {
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  capabilityAccessEpoch,
  subscribeCapabilityAccess,
} from "@/shared/lib/capability-access";
import { providerPresetsApi } from "../api";
import {
  builtinPresetAdvice,
  presetKey,
  type PresetAdvice,
} from "../preset-advice";
import type { ProviderEditorDraft } from "../editor-draft";
import { message } from "../display";
import { useCatalogContext } from "./use-catalog-context";

export function useProviderPresets(
  storeId: string | undefined,
  state: ProviderEditorDraft,
  onApply: (advice: PresetAdvice) => void,
) {
  const { capture, isCurrent, epoch } = useCatalogContext(storeId);
  const access = useSyncExternalStore(
    subscribeCapabilityAccess,
    capabilityAccessEpoch,
  );
  const [items, setItems] =
    useState<readonly PresetAdvice[]>(builtinPresetAdvice);
  const [issues, setIssues] = useState(0);
  const [loaded, setLoaded] = useState(false);
  const [loading, setLoading] = useState(false);
  const [resolving, setResolving] = useState(false);
  const [listError, setListError] = useState<string | null>(null);
  const [selectionError, setSelectionError] = useState<string | null>(null);
  const form = useRef(state),
    serial = useRef(0),
    list = useRef<number | null>(null),
    selected = useRef<number | null>(null);
  useLayoutEffect(() => {
    form.current = state;
  });
  useLayoutEffect(() => {
    list.current = selected.current = null;
    setItems(builtinPresetAdvice);
    setLoaded(false);
    setLoading(false);
    setResolving(false);
    setIssues(0);
    setListError(null);
    setSelectionError(null);
  }, [storeId, epoch, access]);
  const read = useCallback(
    async (force = false) => {
      if (list.current !== null || (loaded && !force)) return;
      const source = capture();
      if (!source) return;
      const token = ++serial.current,
        authority = capabilityAccessEpoch();
      list.current = token;
      setLoading(true);
      setListError(null);
      setSelectionError(null);
      const current = () =>
        list.current === token &&
        isCurrent(source) &&
        capabilityAccessEpoch() === authority;
      try {
        const value = await providerPresetsApi.list(source.storeId);
        if (current()) {
          setItems(value.items);
          setIssues(value.issues.length);
          setLoaded(true);
        }
      } catch (reason) {
        if (current()) {
          setItems(builtinPresetAdvice);
          setIssues(0);
          setLoaded(false);
          setListError(message(reason));
        }
      } finally {
        if (current()) {
          list.current = null;
          setLoading(false);
        }
      }
    },
    [capture, isCurrent, loaded],
  );
  const choose = useCallback(
    async (key: string) => {
      if (selected.current !== null) return;
      const advice = items.find((p) => presetKey(p.origin) === key);
      if (!advice) return;
      setSelectionError(null);
      // The original compiled-in selector remains usable when the optional
      // external metadata service is unavailable. No external choice is guessed.
      if (!loaded && advice.origin.kind === "builtin") {
        onApply(advice);
        return;
      }
      const source = capture();
      if (!source) {
        setSelectionError("请连接草稿原来的后台后核查预设。");
        return;
      }
      const token = ++serial.current,
        originalForm = form.current,
        authority = capabilityAccessEpoch();
      selected.current = token;
      setResolving(true);
      const current = () =>
        selected.current === token &&
        isCurrent(source) &&
        capabilityAccessEpoch() === authority;
      try {
        const resolved = await providerPresetsApi.resolve(
          source.storeId,
          advice.origin,
        );
        if (current() && form.current === originalForm) onApply(resolved);
      } catch (reason) {
        if (current()) {
          setSelectionError(message(reason));
          setLoaded(false);
        }
      } finally {
        if (current()) {
          selected.current = null;
          setResolving(false);
        }
      }
    },
    [items, loaded, capture, isCurrent, onApply],
  );
  const isResolving = useCallback(() => selected.current !== null, []);
  return {
    items,
    issues,
    loaded,
    loading,
    resolving,
    listError,
    selectionError,
    read,
    choose,
    isResolving,
  };
}
