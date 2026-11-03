import { PluginDetailSlots } from "@/features/plugins/detail-slots";
import { ObjectContributionSlots } from "@/features/plugins/object-detail-slots";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { accountsApi } from "@/features/accounts/api";
import {
  accountsForProvider,
  resetCountdown,
  selectBillingAccount,
} from "@/features/accounts/display";
import type { BillingAccount } from "@/features/accounts/types";
import { date } from "@/features/sessions/display";
import { useSharedRead } from "@/shared/hooks/use-shared-read";
import { accountsResource } from "../queries";
import type { ConfigAgentId } from "@/shared/types/agent";
import { ConnectionEmpty } from "@/shared/ui/connection-empty";
import { useCallback, useEffect, useRef, useState } from "react";
import { ProviderQuotaForecast } from "./provider-quota-forecast";
import { ProviderBalanceEstimate } from "./provider-balance-estimate";
const stateLabel: Record<string, string> = {
  not_loaded: "尚未读取",
  unsupported: "读取能力未接入",
  ready: "已读取",
  stale: "快照已过期",
  refreshing: "正在刷新",
  refresh_failed: "刷新失败 · 上次快照",
  failed: "读取失败",
  not_available: "来源未提供额度",
};
export function AccountQuota({
  agentId,
  providerId,
  accountId,
}: {
  agentId?: ConfigAgentId;
  providerId?: string;
  accountId?: string;
}) {
  const resource = useSharedRead(accountsResource);
  const reload = resource.invalidate;
  const [selected, setSelected] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(Date.now());
  const [readConfirm, setReadConfirm] = useState<BillingAccount | null>(null);
  const [collectorOpen, setCollectorOpen] = useState(false);
  const [launcher, setLauncher] = useState<string | null>(null);
  const returnAccount = useRef<BillingAccount | null>(null);
  const scoped = accountsForProvider(resource.data?.accounts ?? [], providerId);
  const rows = accountId
    ? scoped.filter((account) => account.id === accountId)
    : scoped;
  const account = selectBillingAccount(rows, selected, agentId);
  const snapshot = account?.snapshot;
  const snapshotTime = snapshot?.asOf;
  const hasResets = snapshot?.windows.some((w) => w.resetsAt !== null) ?? false;
  useEffect(() => {
    if (!hasResets && !(providerId !== undefined && snapshotTime)) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [hasResets, providerId, snapshotTime]);
  const refresh = useCallback(
    async (a: BillingAccount) => {
      setBusy(true);
      setError(null);
      try {
        await accountsApi.refreshAccount(a.id, a.sourceRevision);
        reload();
        return true;
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : String(reason));
        reload();
        return false;
      } finally {
        setBusy(false);
      }
    },
    [reload],
  );
  useEffect(() => {
    const onFocus = () => {
      const previous = returnAccount.current;
      returnAccount.current = null;
      if (previous?.readable) void refresh(previous);
      else reload();
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh, reload]);
  const openOfficial = async () => {
    if (!account) return;
    setError(null);
    try {
      returnAccount.current = account;
      await accountsApi.openAccountConsole(account.id);
    } catch (reason) {
      returnAccount.current = null;
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  };
  const createCollector = async () => {
    setBusy(true);
    setError(null);
    try {
      const result = await accountsApi.createClaudeQuotaLauncher();
      setLauncher(result.launcherPath);
      void resource.invalidate();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="min-w-0 space-y-4" aria-label="账户用量与额度">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-title font-medium">账户用量与额度</h2>
        </div>
        <Button
          variant="tertiary"
          size="sm"
          onClick={resource.reload}
          loading={resource.loading}
        >
          重新读取目录
        </Button>
      </div>
      {resource.error && (
        <Alert>
          <AlertTitle>账户目录未更新</AlertTitle>
          <AlertDescription>{resource.error}</AlertDescription>
        </Alert>
      )}
      {resource.loading && !resource.data && (
        <p role="status" className="text-body text-fg-muted">
          正在读取账户目录…
        </p>
      )}
      {(rows.length > 1 || (rows.length > 0 && providerId === undefined)) && (
        <Select
          value={account?.id ?? ""}
          disabled={busy}
          onValueChange={(v) => {
            setSelected(String(v));
            setError(null);
          }}
        >
          <SelectTrigger
            aria-label="选择计费账户"
            placeholder="选择账户查看额度"
          />
          <SelectContent>
            {rows.map((a) => (
              <SelectItem key={a.id} value={a.id}>
                {a.accountType === "subscription" ? "订阅" : "API"} · {a.name}
                {agentId && a.declaredFor.includes(agentId)
                  ? " · 文件默认连接"
                  : ""}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      {resource.data && rows.length > 0 && !account && (
        <ConnectionEmpty
          compact
          title="尚未确认此 Agent 的计费账户"
          description="可从账户目录选择查看。未建立账户归属时，不自动展示其他 Agent 的订阅或把模型 Token 换算为余额。"
        />
      )}
      {!resource.loading &&
        !resource.error &&
        resource.data &&
        !rows.length && (
          <ConnectionEmpty
            compact
            title={
              providerId !== undefined
                ? "此连接暂无账户额度数据"
                : "尚无可读取的账户"
            }
            description={
              providerId !== undefined
                ? "此连接没有可关联的计费账户，用量和额度保持未知。"
                : "保存服务商连接或接入本机订阅账户后可查看。云服务商未提供账单接口时，可前往官方控制台。"
            }
          />
        )}
      {account && (
        <>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="min-w-0 space-y-1">
              <h3 className="break-words text-body font-medium">
                {account.name}
              </h3>
              <p className="break-words text-label text-fg-muted">
                {account.scope}
              </p>
              {providerId === undefined && (
                <p className="text-label text-fg-subtle">
                  {agentId && account.declaredFor.includes(agentId)
                    ? "默认账户"
                    : "当前查看账户 · 历史用量归属未知"}
                </p>
              )}
            </div>
            <Badge>{stateLabel[account.state] ?? "状态未知"}</Badge>
          </div>
          <div className="flex flex-wrap gap-2">
            {account.readable && (
              <Button
                size="sm"
                variant="neutral"
                loading={busy}
                onClick={() => {
                  setReadConfirm(account);
                  setError(null);
                }}
              >
                {snapshot ? "刷新账户用量" : "读取真实账户"}
              </Button>
            )}
            {account.officialUrl && (
              <Button
                size="sm"
                variant="tertiary"
                onClick={() => void openOfficial()}
              >
                官方用量与账单
              </Button>
            )}
            {account.id === "native-claude" && (
              <Button
                size="sm"
                variant="neutral"
                onClick={() => {
                  setCollectorOpen(true);
                  setError(null);
                }}
              >
                接入 Claude 额度
              </Button>
            )}
          </div>
          {(error || account.error) && !collectorOpen && !readConfirm && (
            <Alert>
              <AlertTitle>账户数据未更新</AlertTitle>
              <AlertDescription>
                {error ?? account.error}
                {snapshot ? ` 上次读取：${date(snapshot.asOf)}。` : ""}
              </AlertDescription>
            </Alert>
          )}
          {snapshot ? (
            <>
              <p
                className="break-words text-label text-fg-subtle"
                title={snapshot.source}
              >
                更新于 {date(snapshot.asOf)}
              </p>
              {account.accountType === "subscription" ? (
                <>
                  <dl className="grid gap-3 sm:grid-cols-3">
                    <Value
                      label="Plan"
                      value={
                        snapshot.plan
                          ? snapshot.plan[0].toUpperCase() +
                            snapshot.plan.slice(1)
                          : "来源未提供"
                      }
                    />
                    <Value
                      label="账户身份"
                      value={snapshot.identity ?? "来源未提供"}
                    />
                    <Value label="续费 / 到期时间" value="来源未提供" />
                  </dl>
                  {providerId !== undefined ? (
                    <ProviderQuotaForecast
                      key={account.id}
                      account={account}
                      providerId={providerId}
                      now={now}
                      directoryError={!!resource.error}
                    />
                  ) : snapshot.windows.length ? (
                    <div className="grid gap-3 lg:grid-cols-2">
                      {snapshot.windows.map((w) => (
                        <article
                          key={w.id}
                          className="min-w-0 space-y-3 rounded-xl border border-border p-4"
                        >
                          <h4 className="break-words text-body font-medium">
                            {w.label}
                          </h4>
                          <div className="flex flex-wrap justify-between gap-2 text-body">
                            <span>
                              已用{" "}
                              {w.usedPercent === null
                                ? "未知"
                                : `${w.usedPercent}%`}
                            </span>
                            <span>
                              剩余{" "}
                              {w.remainingPercent === null
                                ? "未知"
                                : `${Number(w.remainingPercent.toFixed(2))}%`}
                            </span>
                          </div>
                          {w.usedPercent !== null && (
                            <div
                              className="h-2 w-full overflow-hidden rounded-full bg-muted"
                              role="progressbar"
                              aria-valuemin={0}
                              aria-valuemax={100}
                              aria-valuenow={w.usedPercent}
                              aria-label={`${w.label}已用百分比`}
                            >
                              <div
                                className="h-full bg-fg-default"
                                style={{ width: `${w.usedPercent}%` }}
                              />
                            </div>
                          )}
                          <p className="text-label tabular-nums">
                            {resetCountdown(w.resetsAt, now)}
                          </p>
                          <p className="text-label text-fg-muted">
                            重置时间：
                            {w.resetsAt === null
                              ? "未知"
                              : date(new Date(w.resetsAt * 1000).toISOString())}
                          </p>
                        </article>
                      ))}
                    </div>
                  ) : (
                    <p className="text-body text-fg-muted">
                      该来源未提供额度窗口。
                    </p>
                  )}
                  <article className="space-y-3 rounded-xl border border-border p-4">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <h4 className="text-body font-medium">
                        重置券 ·{" "}
                        {snapshot.resetCredits.availableCount === null
                          ? "数量未知"
                          : `${snapshot.resetCredits.availableCount} 张可用`}
                      </h4>
                      {account.officialUrl && (
                        <Button
                          size="sm"
                          variant="tertiary"
                          onClick={() => void openOfficial()}
                        >
                          {snapshot.resetCredits.availableCount
                            ? "前往官方使用重置券"
                            : "查看官方重置券入口"}
                        </Button>
                      )}
                    </div>
                    <p className="text-label text-fg-muted">
                      {snapshot.resetCredits.reason}
                    </p>
                    {snapshot.resetCredits.expirations.map((expires, i) => (
                      <p key={i} className="text-label text-fg-subtle">
                        已返回券明细 {i + 1} · 有效期{" "}
                        {expires === null
                          ? "未知"
                          : date(new Date(expires * 1000).toISOString())}
                      </p>
                    ))}
                    {!snapshot.resetCredits.detailsComplete && (
                      <p className="text-label text-fg-subtle">
                        明细未完整提供，数量按官方总数显示。
                      </p>
                    )}
                  </article>
                </>
              ) : (
                <>
                  <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
                    {snapshot.money.map((m, i) => (
                      <Value
                        key={`${m.kind}:${m.currency}:${i}`}
                        label={m.label}
                        value={
                          m.amount === null
                            ? "未知"
                            : `${m.currency} ${m.amount}`
                        }
                        description={`${m.period ?? "账期未提供"}${m.resetsAt ? ` · 重置于 ${date(m.resetsAt)}` : ""}`}
                      />
                    ))}
                    {!snapshot.money.some((m) => m.kind === "period_spend") && (
                      <Value label="本期已用金额" value="来源未提供" />
                    )}
                    {!snapshot.money.some((m) => m.kind === "balance") && (
                      <Value label="账户余额" value="来源未提供" />
                    )}
                  </dl>
                  {providerId !== undefined && (
                    <ProviderQuotaForecast
                      key={account.id}
                      account={account}
                      providerId={providerId}
                      now={now}
                      directoryError={!!resource.error}
                    />
                  )}
                </>
              )}
              {snapshot.notes.map((note, i) => (
                <p key={i} className="break-words text-label text-fg-muted">
                  {note}
                </p>
              ))}
            </>
          ) : providerId !== undefined ? (
            <ProviderQuotaForecast
              key={account.id}
              account={account}
              providerId={providerId}
              now={now}
              directoryError={!!resource.error}
            />
          ) : (
            <div className="rounded-xl border border-border p-4 text-body text-fg-muted">
              {account.id === "native-claude"
                ? "通过独立额度入口运行 Claude 后，接收官方提供的 5 小时 / 7 天额度。API 会话可能没有订阅额度。"
                : "尚未取得此账户的用量。读取时只查询账户信息，不发起模型推理。"}
            </div>
          )}
          {providerId !== undefined && account.accountType === "api" && (
            <ProviderBalanceEstimate
              key={`balance:${account.id}`}
              account={account}
              providerId={providerId}
              now={now}
              directoryError={!!resource.error}
            />
          )}
        </>
      )}
      <Dialog
        open={Boolean(readConfirm)}
        onOpenChange={(v) => {
          if (!busy && !v) {
            setReadConfirm(null);
            setError(null);
          }
        }}
      >
        <DialogContent
          size="lg"
          className="max-h-[calc(100dvh-2rem)] overflow-y-auto"
        >
          <DialogHeader>
            <DialogTitle>读取 {readConfirm?.name}</DialogTitle>
            <DialogDescription>
              {readConfirm?.id === "native-codex"
                ? "通过本机 Codex 官方 CLI 查询当前登录账户、Plan、额度窗口和重置券；不会登录、退出、发起推理或消耗重置券。"
                : "使用该连接的凭据，只向已核对的服务商官方账户接口发送读取请求。原生配置内的 API Key 不导入或保存为新凭据。"}
            </DialogDescription>
          </DialogHeader>
          <p className="break-words text-body text-fg-muted">
            {readConfirm?.scope}
          </p>
          {error && (
            <Alert>
              <AlertTitle>读取未完成</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setReadConfirm(null)}
            >
              取消
            </Button>
            <Button
              loading={busy}
              onClick={() => {
                if (readConfirm)
                  void refresh(readConfirm).then((ok) => {
                    if (ok) setReadConfirm(null);
                  });
              }}
            >
              确认读取
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={collectorOpen}
        onOpenChange={(v) => {
          if (!busy) {
            setCollectorOpen(v);
            setError(null);
          }
        }}
      >
        <DialogContent
          size="lg"
          className="max-h-[calc(100dvh-2rem)] overflow-y-auto"
        >
          <DialogHeader>
            <DialogTitle>接入 Claude 官方额度</DialogTitle>
            <DialogDescription>
              为新启动的 Claude 会话生成独立入口，读取官方 statusLine 提供的
              rate_limits。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-3 text-body text-fg-muted">
            <p>
              在项目终端运行此入口，CLI
              参数可追加。该次会话的状态栏显示额度；现有状态栏、原生配置和其他会话保持原设置。
            </p>
            <p>
              仅保存额度百分比、重置时间和采集时间。状态栏未提供的
              Plan、账户身份或券数量显示未知；API /
              第三方会话没有额度时不会补算。
            </p>
            {launcher && (
              <p className="break-all rounded-lg border border-border p-3 font-mono text-label">
                &quot;{launcher}&quot;
              </p>
            )}
            {error && (
              <Alert>
                <AlertTitle>额度入口未创建</AlertTitle>
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}
          </div>
          <DialogFooter>
            <Button
              variant="tertiary"
              disabled={busy}
              onClick={() => setCollectorOpen(false)}
            >
              关闭
            </Button>
            <Button loading={busy} onClick={() => void createCollector()}>
              {launcher ? "重新生成入口" : "生成独立额度入口"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      {account && !resource.error && (
        <>
          <PluginDetailSlots
            key={account.id}
            target="account"
            objectId={account.id}
          />
          <ObjectContributionSlots
            object={{ target: "account", id: account.id }}
          />
        </>
      )}
    </section>
  );
}
function Value({
  label,
  value,
  description,
}: {
  label: string;
  value: string;
  description?: string;
}) {
  return (
    <div className="min-w-0 rounded-xl border border-border p-4">
      <dt className="text-label text-fg-muted">{label}</dt>
      <dd className="mt-2 break-words text-title font-medium tabular-nums">
        {value}
      </dd>
      {description && (
        <p className="mt-2 break-words text-label text-fg-subtle">
          {description}
        </p>
      )}
    </div>
  );
}
