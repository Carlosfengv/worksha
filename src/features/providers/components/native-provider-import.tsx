import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { NativeImportRecovery } from "./native-import-recovery";
import {
  DetailList,
  DetailListItem,
  DetailListLabel,
  DetailListValue,
} from "@/components/ui/detail-list";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { providersApi } from "@/features/providers/api";
import type { ProviderImportCandidate } from "@/features/providers/types";
import {
  forgetNativeImport,
  nativeImportRecovery,
  rememberNativeImport,
  sameNativeImport,
  NATIVE_IMPORT_CHANGED,
  type NativeImportProtocol,
  type NativeImportRequest,
} from "../native-import-operation";
import { useEffect, useRef, useState } from "react";
import {
  backendReadEpoch,
  subscribeBackendRecovery,
} from "@/shared/lib/backend-recovery";

function recoveryRecords() {
  try {
    return nativeImportRecovery();
  } catch {
    return { requests: [] as NativeImportRequest[], invalid: true, count: 1 };
  }
}

export function NativeProviderImport({
  agentId,
  onImported,
}: {
  agentId?: string;
  onImported: () => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [candidates, setCandidates] = useState<ProviderImportCandidate[]>([]);
  const [selected, setSelected] = useState("");
  const [model, setModel] = useState("");
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [protocol, setProtocol] = useState<NativeImportProtocol | null>(null);
  const [serverPending, setServerPending] = useState<NativeImportRequest[]>([]);
  const [recovery, setRecovery] = useState(recoveryRecords);
  const [recoveryMessage, setRecoveryMessage] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState<string | null>(null);
  const busy = useRef(false);
  const generation = useRef(0);
  const scannedEpoch = useRef<symbol | null>(null);
  const [needsScan, setNeedsScan] = useState(false);
  useEffect(() => {
    const changed = () => setRecovery(recoveryRecords());
    window.addEventListener(NATIVE_IMPORT_CHANGED, changed);
    window.addEventListener("storage", changed);
    return () => {
      generation.current++;
      window.removeEventListener(NATIVE_IMPORT_CHANGED, changed);
      window.removeEventListener("storage", changed);
    };
  }, []);
  useEffect(() => {
    if (!open) return;
    return subscribeBackendRecovery(() => {
      generation.current += 1;
      setLoading(false);
      setNeedsScan(true);
      setProtocol(null);
      setConfirmed(null);
    });
  }, [open]);
  const choose = (id: string, list = candidates) => {
    setSelected(id);
    setModel(list.find((c) => c.id === id)?.model ?? "");
    setError(null);
    setConfirmed(null);
  };
  const scan = async () => {
    const current = ++generation.current;
    const epoch = backendReadEpoch();
    setLoading(true);
    setError(null);
    setProtocol(null);
    setConfirmed(null);
    setCandidates([]);
    setSelected("");
    try {
      const preview = await providersApi.previewNativeImport();
      const list = preview.candidates.filter(
        (c) => !agentId || c.agentId === agentId,
      );
      if (current !== generation.current) return;
      if (epoch !== backendReadEpoch()) {
        setNeedsScan(true);
        return;
      }
      scannedEpoch.current = epoch;
      setNeedsScan(false);
      setProtocol(preview.protocol);
      setServerPending(preview.protocol.pendingRequests);
      setCandidates(list);
      choose(
        (
          list.find((c) => c.active && c.canImport) ??
          list.find((c) => c.active) ??
          list[0]
        )?.id ?? "",
        list,
      );
    } catch (reason) {
      if (current === generation.current) {
        if (epoch !== backendReadEpoch()) setNeedsScan(true);
        else
          setError(reason instanceof Error ? reason.message : String(reason));
      }
    } finally {
      if (current === generation.current) setLoading(false);
    }
  };
  const candidate = candidates.find((c) => c.id === selected);
  const pending = new Map(
    recovery.requests.map((request) => [
      request.operation.operationId,
      request,
    ]),
  );
  let recoveryConflict = recovery.invalid;
  for (const request of serverPending) {
    const local = pending.get(request.operation.operationId);
    if (local && !sameNativeImport(local, request)) recoveryConflict = true;
    else pending.set(request.operation.operationId, request);
  }
  const pendingRequests = [...pending.values()];
  const fresh =
    !needsScan &&
    protocol !== null &&
    scannedEpoch.current === backendReadEpoch();
  const capacityFull =
    protocol !== null && protocol.sequence >= protocol.operationLimit;
  const sourceFor = (request: NativeImportRequest) =>
    fresh &&
    protocol?.storeId === request.operation.storeId &&
    protocol.namespaceId === request.operation.namespaceId
      ? candidates.find(
          (c) =>
            c.id === request.candidateId &&
            c.revision === request.expectedRevision &&
            c.canImport,
        )
      : undefined;
  const perform = async (
    request: NativeImportRequest,
    action: "submit" | "resume" | "query" | "cancel",
  ) => {
    if (busy.current) return;
    if (
      action === "resume" &&
      (!sourceFor(request) || confirmed !== request.operation.operationId)
    )
      return;
    busy.current = true;
    const current = generation.current,
      epoch = backendReadEpoch();
    setSaving(true);
    setError(null);
    setRecoveryMessage(null);
    try {
      if (
        action === "resume" &&
        !recoveryRecords().requests.some((saved) =>
          sameNativeImport(saved, request),
        )
      )
        rememberNativeImport(request);
      const result =
        action === "submit" || action === "resume"
          ? await providersApi.importNativeProvider(request)
          : await providersApi.nativeImportResult(request, action === "cancel");
      if (current !== generation.current || epoch !== backendReadEpoch())
        throw new Error("后台已变化，原请求仍保留，请连接原后台核查。");
      if ("status" in result && result.status === "pending") {
        setRecoveryMessage(
          "原导入尚未完成，凭据可能已保存。请核对原来源后明确继续这次导入，或取消导入并保留凭据供独立核查。",
        );
        return;
      }
      let retained = "";
      try {
        forgetNativeImport(request);
      } catch {
        retained = "恢复提示未能移除，请保留原编号再次核查。";
      }
      setServerPending((items) =>
        items.filter(
          (item) =>
            item.operation.operationId !== request.operation.operationId,
        ),
      );
      if ("status" in result && result.status === "cancelled") {
        setRecoveryMessage(
          "已取消原导入；已写入的凭据仍保留在原恢复空间。" + retained,
        );
        return;
      }
      if ("status" in result && result.status === "expired") {
        setRecoveryMessage(
          "原导入请求已过期，后台未接纳这次导入。请重新识别后开始新的导入。" +
            retained,
        );
        setNeedsScan(true);
        setProtocol(null);
        setConfirmed(null);
        return;
      }
      if (!result.provider)
        throw new Error("原导入的已保存结果无法读取，请保留编号核查。");
      const name = result.provider.name;
      setNotice(
        `${result.reused ? "已复用" : "原导入已保存"} ${name}。可在 Agent 详情选择；原 Agent 配置未修改。${retained}`,
      );
      try {
        await onImported();
      } catch {
        setNotice(
          `${name} 已保存，页面刷新未完成。请刷新目录后在 Agent 详情选择。${retained}`,
        );
      }
      setOpen(false);
    } catch (reason) {
      if (current === generation.current) {
        const message =
          reason instanceof Error ? reason.message : String(reason);
        setError(message);
        setRecoveryMessage(message + " 原请求仍保留，请先核查原结果。");
      }
    } finally {
      busy.current = false;
      setSaving(false);
      if (current === generation.current) {
        if (action !== "query") {
          setNeedsScan(true);
          setProtocol(null);
          setConfirmed(null);
        }
      }
    }
  };
  const importSelected = async () => {
    if (
      !candidate?.canImport ||
      !model.trim() ||
      saving ||
      busy.current ||
      !protocol ||
      capacityFull ||
      recoveryConflict ||
      pendingRequests.length > 0 ||
      needsScan ||
      scannedEpoch.current !== backendReadEpoch()
    )
      return;
    const request: NativeImportRequest = {
      candidateId: candidate.id,
      expectedRevision: candidate.revision,
      model: model.trim(),
      operation: {
        storeId: protocol.storeId,
        namespaceId: protocol.namespaceId,
        operationId: crypto.randomUUID(),
        expectedSequence: protocol.sequence,
      },
    };
    try {
      rememberNativeImport(request);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return;
    }
    await perform(request, "submit");
  };
  const recoveryPanel = (
    <NativeImportRecovery
      requests={pendingRequests}
      conflict={recoveryConflict}
      message={recoveryMessage}
      saving={saving}
      loading={loading}
      confirmed={confirmed}
      sourceFor={sourceFor}
      onConfirm={setConfirmed}
      onSource={() => {
        setOpen(true);
        void scan();
      }}
      onAction={(request, action) => void perform(request, action)}
    />
  );
  return (
    <div className="space-y-3">
      <Button
        variant="tertiary"
        size="sm"
        disabled={saving}
        onClick={() => {
          setOpen(true);
          setNotice(null);
          void scan();
        }}
      >
        导入本机配置
      </Button>
      {notice && (
        <Alert>
          <AlertTitle>本机配置导入结果</AlertTitle>
          <AlertDescription>{notice}</AlertDescription>
        </Alert>
      )}
      {!open && recoveryPanel}
      <Dialog
        open={open}
        onOpenChange={(value) => {
          if (saving) return;
          setOpen(value);
          if (!value) {
            generation.current++;
            setLoading(false);
          }
        }}
      >
        <DialogContent
          size="lg"
          className="max-h-[calc(100dvh-2rem)] overflow-y-auto"
        >
          <DialogHeader>
            <DialogTitle>导入本机 Provider 配置</DialogTitle>
            <DialogDescription>
              识别 Claude Code / Codex 的用户级配置。API Key
              由后台直接存入钥匙串，不显示在界面；不改写原 Agent
              配置，不发送模型请求。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            {open && recoveryPanel}
            {capacityFull && (
              <Alert status="warning">
                <AlertTitle>导入记录容量已满</AlertTitle>
                <AlertDescription>
                  原操作记录和凭据已保留，可以继续核查原编号，新的导入暂不可提交。
                </AlertDescription>
              </Alert>
            )}
            {needsScan && (
              <p role="status" className="text-body text-fg-muted">
                请重新识别并核对导入目标；输入已保留。
              </p>
            )}
            {error && (
              <Alert status="danger">
                <AlertTitle>导入未完成</AlertTitle>
                <AlertDescription>
                  {error}。已有原操作时请查询其结果。
                </AlertDescription>
              </Alert>
            )}
            {loading ? (
              <p className="text-body text-fg-muted" role="status">
                正在识别本机配置…
              </p>
            ) : candidates.length > 0 ? (
              <>
                <Field>
                  <FieldLabel>配置来源</FieldLabel>
                  <Select
                    value={selected}
                    onValueChange={(value) => choose(String(value))}
                    disabled={saving}
                  >
                    <SelectTrigger aria-label="导入配置来源" />
                    <SelectContent>
                      {candidates.map((c) => (
                        <SelectItem key={c.id} value={c.id}>
                          {c.name}
                          {c.active ? " · 当前用户配置" : " · 已定义"}
                          {c.existingProviderId ? " · 已在 Providers" : ""}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <FieldDescription>
                    仅识别默认用户目录；项目设置、CLI 覆盖和独立 Profile
                    不在此次范围内。
                  </FieldDescription>
                </Field>
                {candidate && (
                  <>
                    <div className="flex flex-wrap gap-2">
                      <Badge
                        status={
                          candidate.canImport && !needsScan
                            ? "neutral"
                            : "warning"
                        }
                      >
                        {needsScan
                          ? "待重新识别"
                          : candidate.existingProviderId
                            ? "已在 Providers"
                            : candidate.canImport
                              ? "可导入"
                              : "需要核查"}
                      </Badge>
                      <Badge>
                        {candidate.agentId === "claude"
                          ? "Claude Code"
                          : "Codex"}
                      </Badge>
                    </div>
                    <DetailList>
                      <DetailListItem>
                        <DetailListLabel>配置文件</DetailListLabel>
                        <DetailListValue className="break-all">
                          {candidate.sourcePath}
                        </DetailListValue>
                      </DetailListItem>
                      <DetailListItem>
                        <DetailListLabel>Endpoint</DetailListLabel>
                        <DetailListValue className="break-all">
                          {candidate.endpoint ?? "未识别"}
                        </DetailListValue>
                      </DetailListItem>
                      <DetailListItem>
                        <DetailListLabel>接口协议</DetailListLabel>
                        <DetailListValue>{candidate.protocol}</DetailListValue>
                      </DetailListItem>
                      <DetailListItem>
                        <DetailListLabel>凭据来源</DetailListLabel>
                        <DetailListValue className="break-all">
                          {candidate.credentialSource}
                        </DetailListValue>
                      </DetailListItem>
                    </DetailList>
                    {candidate.model ? (
                      <p className="text-body">默认模型：{candidate.model}</p>
                    ) : (
                      candidate.canImport && (
                        <Field>
                          <FieldLabel>默认模型</FieldLabel>
                          <Input
                            aria-label="导入连接默认模型"
                            value={model}
                            onChange={(event) => setModel(event.target.value)}
                            placeholder="此 Provider 未指定模型，请补充模型 ID"
                            disabled={saving}
                          />
                          <FieldDescription>
                            非当前 Codex Provider
                            不继承当前模型，避免把其他服务商模型误绑定。
                          </FieldDescription>
                        </Field>
                      )
                    )}
                    {candidate.reason && (
                      <Alert status="warning">
                        <AlertDescription>{candidate.reason}</AlertDescription>
                      </Alert>
                    )}
                  </>
                )}
              </>
            ) : (
              !error &&
              !needsScan && (
                <p className="text-body text-fg-muted">
                  没有发现可识别的本机配置。
                </p>
              )
            )}
            <p className="text-label text-fg-subtle">
              订阅登录由 Agent
              管理，仍可在详情选择官方原生登录。外部凭据助手不会执行；导入不代表协议、模型或余额已经验证。
            </p>
          </div>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={saving || loading}
              onClick={() => void scan()}
            >
              重新识别
            </Button>
            <Button
              variant="tertiary"
              disabled={saving}
              onClick={() => {
                generation.current++;
                setOpen(false);
              }}
            >
              取消
            </Button>
            <Button
              variant="neutral"
              loading={saving}
              disabled={
                loading ||
                saving ||
                needsScan ||
                !protocol ||
                capacityFull ||
                recoveryConflict ||
                pendingRequests.length > 0 ||
                !candidate?.canImport ||
                !model.trim()
              }
              onClick={() => void importSelected()}
            >
              {candidate?.existingProviderId
                ? "复用已有连接"
                : "导入到 Providers"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
