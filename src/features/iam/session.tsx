import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { settingsApi } from "@/features/settings/api";
import {
  invalidateBackendReads,
  subscribeBackendRecovery,
} from "@/shared/lib/backend-recovery";
import { iamApi } from "./api";
import { IamContext } from "./context";
import { CredentialFields, PasswordField } from "./credentials";
import { field, useIamAction } from "./actions";
import type { Account, IamStatus, SetupPermit } from "./types";

function RecoveryCode({ permit }: { permit: SetupPermit }) {
  return (
    <div className="space-y-2 rounded-lg border border-border p-3">
      <p className="font-medium">保存 Owner 恢复码</p>
      <p className="text-label text-fg-muted">
        忘记 Owner 密码时使用。请把恢复码保存在安全位置。
      </p>
      <code className="block select-all break-all text-label">
        {permit.recoveryCode}
      </code>
      <label className="flex items-center gap-2 text-label">
        <Checkbox name="saved" required />
        我已保存恢复码
      </label>
    </div>
  );
}

function FirstSetup({
  onClose,
  onFinished,
}: {
  onClose: () => void;
  onFinished: () => Promise<void>;
}) {
  const [permit, setPermit] = useState<SetupPermit | null>(null);
  const task = useIamAction();
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !task.busy) onClose();
      }}
    >
      <DialogContent className="max-h-[85dvh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>开启账号管理</DialogTitle>
          <DialogDescription>
            完成设置后，此实例始终要求账号登录。账号管理插件无法关闭或卸载。
          </DialogDescription>
        </DialogHeader>
        <form
          className="space-y-4"
          onSubmit={(event) => {
            event.preventDefault();
            const values = new FormData(event.currentTarget);
            void task.run(async () => {
              if (!permit) {
                setPermit(await iamApi.prepare(false));
                return;
              }
              if (!values.has("saved")) throw new Error("请先保存恢复码。");
              try {
                await iamApi.initialize(permit.operationId, {
                  username: field(values, "username"),
                  displayName: field(values, "displayName"),
                  password: field(values, "password"),
                });
                onClose();
              } finally {
                await onFinished();
              }
            });
          }}
        >
          {permit && (
            <>
              <CredentialFields mode="setup" />
              <RecoveryCode permit={permit} />
            </>
          )}
          {task.error && (
            <Alert status="danger">
              <AlertDescription>{task.error}</AlertDescription>
            </Alert>
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="secondary"
              onClick={onClose}
              disabled={task.busy}
            >
              取消
            </Button>
            <Button type="submit" disabled={task.busy}>
              {task.busy ? "正在处理…" : permit ? "创建并开启" : "继续设置"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function SignIn({ onFinished }: { onFinished: () => Promise<void> }) {
  const [recovery, setRecovery] = useState<SetupPermit | null>(null);
  const [recovered, setRecovered] = useState(false);
  const task = useIamAction();
  return (
    <div className="flex h-dvh flex-col bg-surface-base">
      <div className="h-9 shrink-0" data-tauri-drag-region />
      <main className="flex min-h-0 flex-1 overflow-y-auto px-6 py-8">
        <section className="m-auto w-full max-w-md space-y-5">
          <div>
            <p className="text-label text-fg-muted">Local Agent</p>
            <h1 className="text-title font-medium">
              {recovery ? "恢复 Owner 账号" : "登录账号"}
            </h1>
          </div>
          {recovered && (
            <Alert>
              <AlertDescription>
                密码已重置。请使用新密码登录。
              </AlertDescription>
            </Alert>
          )}
          <form
            key={recovery?.operationId ?? "login"}
            className="space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              const values = new FormData(event.currentTarget);
              void task.run(async () => {
                if (recovery) {
                  if (!values.has("saved"))
                    throw new Error("请先保存新恢复码。");
                  await iamApi.recover(
                    recovery.operationId,
                    field(values, "recoveryCode").trim(),
                    field(values, "password"),
                  );
                  setRecovered(true);
                  setRecovery(null);
                } else {
                  await iamApi.login(
                    field(values, "username"),
                    field(values, "password"),
                  );
                }
                await onFinished();
              });
            }}
          >
            {recovery ? (
              <>
                <div className="space-y-1.5">
                  <label
                    className="text-label font-medium"
                    htmlFor="iam-recovery-code"
                  >
                    原恢复码
                  </label>
                  <Input
                    id="iam-recovery-code"
                    name="recoveryCode"
                    required
                    maxLength={64}
                    autoComplete="off"
                  />
                </div>
                <PasswordField name="password" label="新密码" />
                <RecoveryCode permit={recovery} />
              </>
            ) : (
              <CredentialFields mode="login" />
            )}
            {task.error && (
              <Alert status="danger">
                <AlertDescription>{task.error}</AlertDescription>
              </Alert>
            )}
            <div className="flex flex-wrap gap-2">
              <Button type="submit" disabled={task.busy}>
                {task.busy ? "正在验证…" : recovery ? "重置密码" : "登录"}
              </Button>
              <Button
                type="button"
                variant="tertiary"
                disabled={task.busy}
                onClick={() => {
                  void task.run(async () => {
                    if (recovery) setRecovery(null);
                    else {
                      setRecovered(false);
                      setRecovery(await iamApi.prepare(true));
                    }
                  });
                }}
              >
                {recovery ? "返回登录" : "恢复 Owner 账号"}
              </Button>
            </div>
          </form>
        </section>
      </main>
    </div>
  );
}

export function IamSession({
  children,
  member,
}: {
  children: ReactNode;
  member: (account: Account) => ReactNode;
}) {
  const [status, setStatus] = useState<IamStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [setup, setSetup] = useState(false);
  const revision = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++revision.current;
    try {
      const next = await iamApi.status();
      if (request === revision.current) {
        setStatus(next);
        setError(null);
      }
    } catch (reason) {
      if (request === revision.current) {
        setError(reason instanceof Error ? reason.message : "账号状态暂不可用");
      }
    }
  }, []);
  useEffect(() => {
    void refresh();
    const reload = () => {
      void refresh();
    };
    const stop = subscribeBackendRecovery(reload);
    window.addEventListener("focus", reload);
    window.addEventListener("local-agent:login-required", reload);
    const timer = window.setInterval(reload, 30_000);
    return () => {
      stop();
      window.clearInterval(timer);
      window.removeEventListener("focus", reload);
      window.removeEventListener("local-agent:login-required", reload);
    };
  }, [refresh]);
  const transition = async () => {
    await refresh();
    invalidateBackendReads();
  };
  const logout = async () => {
    try {
      await iamApi.logout();
    } finally {
      await transition();
    }
  };
  const reconnect = useIamAction();
  let view: ReactNode;
  if (error || !status) {
    view = (
      <main className="flex h-dvh items-center justify-center p-6">
        <div className="space-y-3">
          {error ? (
            <>
              <Alert status="danger">
                <AlertDescription>{reconnect.error ?? error}</AlertDescription>
              </Alert>
              <Button variant="secondary" onClick={() => void refresh()}>
                重新读取
              </Button>
              <Button
                variant="tertiary"
                disabled={reconnect.busy}
                onClick={() => void reconnect.run(settingsApi.reconnectBackend)}
              >
                重新连接后台
              </Button>
            </>
          ) : (
            <p role="status" className="text-fg-muted">
              正在读取登录状态…
            </p>
          )}
        </div>
      </main>
    );
  } else if (status.enabled && !status.account) {
    view = <SignIn onFinished={transition} />;
  } else if (status.account?.role === "member") {
    view = member(status.account);
  } else {
    view = children;
  }
  return (
    <IamContext.Provider
      value={{
        status,
        beginSetup:
          status?.available && !status.enabled
            ? () => setSetup(true)
            : undefined,
        logout,
        refresh,
      }}
    >
      <div key={status?.account?.id ?? (status?.enabled ? "login" : "local")}>
        {view}
      </div>
      {setup && status && !status.enabled && (
        <FirstSetup onClose={() => setSetup(false)} onFinished={transition} />
      )}
    </IamContext.Provider>
  );
}
