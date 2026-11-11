import { useRef, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import { DataTable, useDataTable } from "@/components/ui/data-table";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabItem, TabPanel, TabsList } from "@/components/ui/tabs";
import { AccountQuota } from "@/features/accounts/components/account-quota";
import { routingQuotaLabels } from "@/features/accounts/display";
import { useSharedQuery } from "@/shared/hooks/use-shared-read";
import { formatDateTime } from "@/shared/lib/date-time";
import { providersApi } from "../api";
import { availableProviderModelIds } from "../catalog";
import { message } from "../display";
import { useCredentialOperations } from "../hooks/use-credential-operations";
import { upstreamAccountsResource } from "../queries";
import {
  cooldownLabels,
  recoveryLabels,
  verificationLabels,
} from "../upstream-status";
import type {
  Provider,
  UpstreamAccount,
  UpstreamAccountAction,
  UpstreamCooldown,
  UpstreamRecoveryState,
} from "../types";
import { CredentialNamespaceRecovery } from "./credential-namespace-recovery";
import { CredentialRecovery } from "./credential-recovery";

type Edit = {
  id: string;
  expectedRevision: number;
  label: string;
  providerRevision: number;
};
const labels = { enabled: "已启用", paused: "已暂停", revoked: "已撤销" };
type Row = UpstreamAccount & { actions: React.ReactNode };
function routingStatus(account: UpstreamAccount) {
  const statuses = new Set<string>();
  if (account.routingQuota)
    statuses.add(routingQuotaLabels[account.routingQuota]);
  if (account.cooldowns == null) statuses.add("冷却状态未知");
  else if (account.cooldowns.length) statuses.add("有候选处于冷却");
  for (const recovery of account.recoveries ?? []) {
    statuses.add(recoveryLabels[recovery.state]);
  }
  return statuses.size ? [...statuses].join(" · ") : "无活跃冷却";
}

const columns: ColumnDef<Row>[] = [
  { accessorKey: "label", header: "上游账号" },
  {
    id: "state",
    header: "路由状态",
    cell: ({ row: { original: account } }) => (
      <div>
        <p>{labels[account.state]}</p>
        {account.state === "enabled" && (
          <p className="text-label text-fg-muted">{routingStatus(account)}</p>
        )}
      </div>
    ),
  },
  {
    id: "verification",
    header: "最近验证",
    cell: ({ row: { original: account } }) => {
      const latest = account.verifications?.[0];
      return latest ? (
        <div className="max-w-48 break-all">
          <p>{latest.model}</p>
          <p className="text-label text-fg-muted">
            {verificationLabels[latest.state]} ·{" "}
            {formatDateTime(latest.checkedAt)}
          </p>
        </div>
      ) : account.verifications == null ? (
        "验证记录未知"
      ) : (
        "尚未验证"
      );
    },
  },
  { accessorKey: "revision", header: "版本" },
  { id: "actions", header: "操作", cell: ({ row }) => row.original.actions },
];

function AccountInspection({
  account,
  current,
  provider,
  storeId,
  onClose,
}: {
  account: UpstreamAccount;
  current?: UpstreamAccount;
  provider: Provider;
  storeId: string;
  onClose: () => void;
}) {
  const models = availableProviderModelIds(provider);
  const cooldowns:
    | (UpstreamCooldown & { state?: UpstreamRecoveryState })[]
    | null
    | undefined = current?.cooldowns && [
    ...current.cooldowns,
    ...(current.recoveries ?? []),
  ];
  const [model, setModel] = useState(models[0] ?? "");
  const [tab, setTab] = useState("verification");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const canVerify =
    provider.configured &&
    current?.state === "enabled" &&
    models.includes(model);
  const verify = async () => {
    if (!canVerify || busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const result = await providersApi.upstreamAccount(provider.id, {
        kind: "verify",
        storeId,
        expectedRevision: provider.revision,
        account: { id: account.id, revision: account.revision },
        model,
      });
      const updated = result.accounts.find(
        (a) => a.id === account.id && a.revision === account.revision,
      );
      const check = updated?.verifications?.find(
        (entry) => entry.model === model,
      );
      if (check?.state === "verified") setNotice(`${model} 验证通过。`);
      else if (check?.state === "failed")
        setError(`${model} 验证失败，请核对连接后重试。`);
      else setError("无法核查本次验证结果，请刷新后重试。");
    } catch (reason) {
      setError(message(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !busy) onClose();
      }}
    >
      <DialogContent size="lg" className="max-h-[85dvh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>验证与状态 · {account.label}</DialogTitle>
          <DialogDescription>
            验证会用此账号向选定模型发送一次短请求，可能产生用量。
          </DialogDescription>
        </DialogHeader>
        {!current && (
          <Alert status="warning">
            <AlertDescription>
              当前账号或连接已变化，请关闭后重新读取。
            </AlertDescription>
          </Alert>
        )}
        <Tabs
          value={tab}
          onValueChange={setTab}
          variant="underline"
          color="default"
        >
          <TabsList aria-label="上游账号详情">
            <TabItem value="verification" label="验证与冷却" />
            <TabItem value="quota" label="用量与额度" />
          </TabsList>
          <TabPanel value="verification" className="space-y-4">
            {current?.state === "enabled" && (
              <Field>
                <FieldLabel htmlFor="upstream-verification-model">
                  验证模型
                </FieldLabel>
                <Combobox
                  items={models}
                  value={models.includes(model) ? model : null}
                  limit={200}
                  disabled={busy || !provider.configured || !models.length}
                  onValueChange={(value) => {
                    if (value && models.includes(value)) {
                      setModel(value);
                      setError(null);
                      setNotice(null);
                    }
                  }}
                >
                  <ComboboxInput
                    id="upstream-verification-model"
                    placeholder="搜索已启用的模型 ID"
                  />
                  <ComboboxContent>
                    <ComboboxEmpty>没有匹配的已启用模型。</ComboboxEmpty>
                    <ComboboxList>
                      {(id: string) => (
                        <ComboboxItem key={id} value={id}>
                          {id}
                        </ComboboxItem>
                      )}
                    </ComboboxList>
                  </ComboboxContent>
                </Combobox>
                {!models.length && (
                  <p className="text-label text-fg-muted">
                    请先启用要验证的模型。
                  </p>
                )}
              </Field>
            )}
            {error && (
              <Alert status="danger">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
            {current && notice && (
              <p role="status" className="text-body">
                {notice}
              </p>
            )}
            <section className="space-y-2" aria-label="模型验证记录">
              <h3 className="text-body font-medium">模型验证记录</h3>
              {current?.verifications == null ? (
                <p className="text-body text-fg-muted">验证记录未知。</p>
              ) : current.verifications.length ? (
                <ul className="space-y-2 text-body">
                  {current.verifications.map((entry) => (
                    <li key={entry.model} className="break-all">
                      <p>{entry.model}</p>
                      <p className="text-label text-fg-muted">
                        {verificationLabels[entry.state]} ·{" "}
                        {formatDateTime(entry.checkedAt, "full")}
                      </p>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="text-body text-fg-muted">尚无验证记录。</p>
              )}
            </section>
            <section className="space-y-2" aria-label="冷却记录">
              <h3 className="text-body font-medium">冷却记录</h3>
              {current?.routingQuota && (
                <p className="text-body">
                  {routingQuotaLabels[current.routingQuota]}
                </p>
              )}
              {cooldowns == null ? (
                <p className="text-body text-fg-muted">
                  {current && current.state !== "enabled"
                    ? labels[current.state]
                    : "冷却状态未知。"}
                </p>
              ) : cooldowns.length ? (
                <ul className="space-y-2 text-body">
                  {cooldowns.map((entry) => (
                    <li key={entry.model ?? ""} className="break-all">
                      <p>{entry.model ?? "整个账号"}</p>
                      <p className="text-label text-fg-muted">
                        {cooldownLabels[entry.reason] ?? "冷却原因未知"} ·{" "}
                        {entry.state
                          ? recoveryLabels[entry.state]
                          : `至 ${formatDateTime(entry.untilMs, "full")}`}
                      </p>
                    </li>
                  ))}
                </ul>
              ) : !current?.routingQuota ? (
                <p className="text-body text-fg-muted">无活跃冷却。</p>
              ) : null}
            </section>
          </TabPanel>
          <TabPanel value="quota">
            {tab === "quota" &&
              current &&
              (current.state === "enabled" ? (
                <AccountQuota
                  providerId={provider.id}
                  upstreamAccount={{
                    id: current.id,
                    revision: current.revision,
                  }}
                />
              ) : (
                <p className="text-body text-fg-muted">
                  {labels[current.state]}，额度读取不可用。
                </p>
              ))}
          </TabPanel>
        </Tabs>
        <DialogFooter>
          <Button variant="tertiary" disabled={busy} onClick={onClose}>
            关闭
          </Button>
          {tab === "verification" && current?.state === "enabled" && (
            <Button
              loading={busy}
              disabled={!canVerify || busy}
              onClick={() => void verify()}
            >
              开始验证
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function UpstreamAccounts({
  provider,
  storeId,
}: {
  provider: Provider;
  storeId?: string;
}) {
  const query = useSharedQuery(upstreamAccountsResource, provider.id);
  const current = !!storeId && query.data?.storeId === storeId;
  const credentials = useCredentialOperations(
    storeId,
    current ? storeId : null,
  );
  const [edit, setEdit] = useState<Edit | null>(null);
  const [useDefault, setUseDefault] = useState(false);
  const [reference, setReference] = useState<string | null>(null);
  const [revoke, setRevoke] = useState<UpstreamAccount | null>(null);
  const [inspection, setInspection] = useState<{
    account: UpstreamAccount;
    storeId: string;
    providerRevision: number;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const secret = useRef<HTMLInputElement>(null);
  const saving = busy || credentials.busy;
  const allowed = current && !query.error && provider.configured;

  const open = (account?: UpstreamAccount) => {
    setError(null);
    setReference(null);
    setUseDefault(false);
    setEdit({
      id: account?.id ?? crypto.randomUUID(),
      expectedRevision: account?.revision ?? 0,
      label: account?.label ?? "",
      providerRevision: provider.revision,
    });
  };
  const run = async (work: () => Promise<void>) => {
    if (!allowed || saving) return;
    setBusy(true);
    setError(null);
    try {
      await work();
    } catch (reason) {
      setError(message(reason));
    } finally {
      setBusy(false);
    }
  };
  const save = () =>
    run(async () => {
      if (!edit || !storeId) return;
      const label = edit.label.trim();
      if (!label) throw Error("请输入账号名称。");
      let credentialRef = useDefault ? provider.credentialRef : reference;
      if (!credentialRef) {
        const key = secret.current?.value.trim() ?? "";
        if (provider.vendor === "minimax-plan" && !/^sk-cp-[!-~]+$/.test(key))
          throw Error("请填写 MiniMax M Plan 的 sk-cp- 订阅 Key。");
        if (secret.current) secret.current.value = "";
        const receipt = await credentials.create(label, key);
        credentialRef = receipt.credentialRef;
        setReference(credentialRef);
      }
      const result = await providersApi.upstreamAccount(provider.id, {
        kind: "save",
        storeId,
        account: { ...edit, label, credentialRef },
      });
      if (
        !result.accounts.some(
          (a) =>
            a.id === edit.id &&
            a.revision === edit.expectedRevision + 1 &&
            a.label === label,
        )
      ) {
        throw Error("上游账号保存结果待核查，请按原内容重试。");
      }
      credentials.confirmBound(credentialRef);
      setEdit(null);
      setReference(null);
    });
  const changeState = (
    account: UpstreamAccount,
    state: UpstreamAccount["state"],
  ) =>
    run(async () => {
      if (!storeId) return;
      const action: UpstreamAccountAction = {
        kind: "set-state",
        storeId,
        account: { id: account.id, revision: account.revision },
        expectedState: account.state,
        state,
      };
      const result = await providersApi.upstreamAccount(provider.id, action);
      if (
        !result.accounts.some((a) => a.id === account.id && a.state === state)
      ) {
        throw Error("账号状态尚未确认，请刷新后核查。");
      }
      setRevoke(null);
    });
  const rows: Row[] = (current ? (query.data?.accounts ?? []) : []).map(
    (a) => ({
      ...a,
      actions: (
        <div className="flex flex-wrap gap-2">
          <Button
            size="sm"
            variant="tertiary"
            aria-label={`验证与状态 ${a.label}`}
            disabled={!storeId || saving}
            onClick={() => {
              if (!storeId) return;
              setError(null);
              setInspection({
                account: a,
                storeId,
                providerRevision: provider.revision,
              });
            }}
          >
            验证与状态
          </Button>
          {a.state !== "revoked" && (
            <>
              <Button
                size="sm"
                variant="tertiary"
                disabled={!allowed || saving}
                onClick={() => open(a)}
              >
                更换 Key
              </Button>
              <Button
                size="sm"
                variant="tertiary"
                disabled={!allowed || saving}
                onClick={() =>
                  void changeState(
                    a,
                    a.state === "enabled" ? "paused" : "enabled",
                  )
                }
              >
                {a.state === "enabled" ? "暂停" : "恢复"}
              </Button>
              <Button
                size="sm"
                variant="tertiary"
                disabled={!allowed || saving}
                onClick={() => {
                  setError(null);
                  setRevoke(a);
                }}
              >
                撤销
              </Button>
            </>
          )}
        </div>
      ),
    }),
  );
  const { table } = useDataTable({
    data: rows,
    columns,
    getRowId: (row) => row.id,
    manualPagination: true,
    enableSorting: false,
    enableHiding: false,
    enableRowSelection: false,
  });

  return (
    <section className="space-y-4" aria-label="上游账号">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-title font-medium">上游账号</h3>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="tertiary"
            disabled={query.loading || saving}
            onClick={query.reload}
          >
            刷新状态
          </Button>
          <Button
            variant="secondary"
            disabled={!allowed || saving || rows.length >= 32}
            onClick={() => open()}
          >
            添加账号
          </Button>
        </div>
      </div>
      <p className="text-body text-fg-muted">
        把账号加入模型服务后，发布时会用它验证所选模型。暂停和撤销影响使用该账号的模型服务。
      </p>
      {query.error || (query.data && !current) ? (
        <Alert status="warning">
          <AlertDescription>
            账号目录无法核查。请刷新原后台后重试。
          </AlertDescription>
        </Alert>
      ) : query.loading ? (
        <p role="status">正在读取上游账号…</p>
      ) : current && rows.length === 0 ? (
        <p className="text-body text-fg-muted">还没有上游账号。</p>
      ) : rows.length > 0 ? (
        <DataTable table={table} showPagination={false} />
      ) : null}
      {error && !edit && !revoke && !inspection && (
        <Alert status="danger">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      {inspection && (
        <AccountInspection
          account={inspection.account}
          current={
            current &&
            !query.error &&
            storeId === inspection.storeId &&
            provider.revision === inspection.providerRevision
              ? query.data?.accounts.find(
                  (a) =>
                    a.id === inspection.account.id &&
                    a.revision === inspection.account.revision,
                )
              : undefined
          }
          provider={provider}
          storeId={inspection.storeId}
          onClose={() => setInspection(null)}
        />
      )}
      {edit && (
        <Dialog
          open
          onOpenChange={(open) => {
            if (!open && !saving) setEdit(null);
          }}
        >
          <DialogContent size="lg" className="max-h-[85dvh] overflow-y-auto">
            <DialogHeader>
              <DialogTitle>
                {edit.expectedRevision ? "更换上游账号 Key" : "添加上游账号"}
              </DialogTitle>
              <DialogDescription>
                {edit.expectedRevision
                  ? "更换后，使用此账号的模型服务需要采用新版本并重新发布。"
                  : "可以登记默认连接的凭据，或为此 Provider 单独添加 API Key。"}
              </DialogDescription>
            </DialogHeader>
            <div className="space-y-3">
              {error && (
                <Alert status="danger">
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              )}
              <Field>
                <FieldLabel htmlFor="upstream-account-label">
                  账号名称
                </FieldLabel>
                <Input
                  id="upstream-account-label"
                  maxLength={80}
                  value={edit.label}
                  disabled={saving || !!reference}
                  onChange={(e) => setEdit({ ...edit, label: e.target.value })}
                />
              </Field>
              {!edit.expectedRevision &&
                provider.credentialRef &&
                !reference && (
                  <Switch
                    label="使用 Provider 默认凭据"
                    checked={useDefault}
                    disabled={saving}
                    onCheckedChange={setUseDefault}
                  />
                )}
              {!useDefault && (
                <>
                  {reference ? (
                    <p className="text-body text-fg-muted">
                      凭据已保存，可以继续登记账号。
                    </p>
                  ) : (
                    <Field>
                      <FieldLabel htmlFor="upstream-account-key">
                        {provider.vendor === "minimax-plan"
                          ? "M Plan 订阅 Key"
                          : "API Key"}
                      </FieldLabel>
                      <Input
                        id="upstream-account-key"
                        ref={secret}
                        type="password"
                        autoComplete="off"
                        disabled={saving}
                      />
                    </Field>
                  )}
                  <CredentialNamespaceRecovery
                    operations={credentials}
                    disabled={saving}
                  />
                  {credentials.records.length > 0 && (
                    <CredentialRecovery
                      operations={credentials}
                      currentReference={reference}
                      disabled={saving}
                      readSecret={() => {
                        const key = secret.current?.value.trim() ?? "";
                        if (secret.current) secret.current.value = "";
                        return key;
                      }}
                      onAdopt={(receipt) => {
                        setReference(receipt.credentialRef);
                        setEdit({ ...edit, label: receipt.credentialLabel });
                      }}
                    />
                  )}
                </>
              )}
            </div>
            <DialogFooter>
              <Button
                variant="tertiary"
                disabled={saving}
                onClick={() => setEdit(null)}
              >
                关闭
              </Button>
              <Button
                loading={saving}
                disabled={!allowed || saving || !edit.label.trim()}
                onClick={() => void save()}
              >
                保存账号
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
      {revoke && (
        <Dialog
          open
          onOpenChange={(open) => {
            if (!open && !saving) setRevoke(null);
          }}
        >
          <DialogContent>
            <DialogHeader>
              <DialogTitle>撤销上游账号</DialogTitle>
              <DialogDescription>
                「{revoke.label}」会从新请求中停用。历史尝试记录会保留。
              </DialogDescription>
            </DialogHeader>
            {error && (
              <Alert status="danger">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
            <DialogFooter>
              <Button
                variant="tertiary"
                disabled={saving}
                onClick={() => setRevoke(null)}
              >
                取消
              </Button>
              <Button
                loading={saving}
                disabled={saving}
                onClick={() => void changeState(revoke, "revoked")}
              >
                撤销账号
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      )}
    </section>
  );
}
