import { useId, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { iamApi } from "./api";
import { field, useIamAction } from "./actions";
import { CredentialFields, PasswordField } from "./credentials";
import { useIam } from "./context";
import type { Account, Role } from "./types";

export function AccountDialog({
  account,
  onClose,
  onSaved,
}: {
  account?: Account;
  onClose: () => void;
  onSaved: (account: Account) => Promise<void>;
}) {
  const id = useId();
  const [role, setRole] = useState<Role>(account?.role ?? "member");
  const [enabled, setEnabled] = useState(account?.enabled ?? true);
  const task = useIamAction();
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !task.busy) onClose();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{account ? "编辑账号" : "新建账号"}</DialogTitle>
          <DialogDescription>
            {account ? account.username : "为成员设置登录信息和内置角色。"}
          </DialogDescription>
        </DialogHeader>
        <form
          className="space-y-4"
          onSubmit={(event) => {
            event.preventDefault();
            const data = new FormData(event.currentTarget);
            const displayName = field(data, "displayName");
            void task.run(async () => {
              const saved = account
                ? await iamApi.edit(account.id, {
                    displayName,
                    role,
                    enabled,
                    revision: account.revision,
                  })
                : await iamApi.create(
                    {
                      displayName,
                      username: field(data, "username"),
                      password: field(data, "password"),
                    },
                    role,
                  );
              await onSaved(saved);
              onClose();
            });
          }}
        >
          {account ? (
            <div className="space-y-1.5">
              <label className="text-label font-medium" htmlFor={id + "name"}>
                显示名
              </label>
              <Input
                id={id + "name"}
                name="displayName"
                defaultValue={account.displayName}
                required
                maxLength={64}
              />
            </div>
          ) : (
            <CredentialFields mode="create" />
          )}
          <div className="space-y-1.5">
            <label className="text-label font-medium" htmlFor={id + "role"}>
              角色
            </label>
            <Select
              value={role}
              onValueChange={(value) => {
                if (value === "owner" || value === "member") setRole(value);
              }}
            >
              <SelectTrigger id={id + "role"} />
              <SelectContent>
                <SelectItem value="owner">Owner · 管理员</SelectItem>
                <SelectItem value="member">Member · 成员</SelectItem>
              </SelectContent>
            </Select>
          </div>
          {account && (
            <Switch
              label="启用账号"
              checked={enabled}
              onCheckedChange={setEnabled}
            />
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
              disabled={task.busy}
              onClick={onClose}
            >
              取消
            </Button>
            <Button type="submit" disabled={task.busy}>
              {task.busy ? "正在保存…" : "保存账号"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function PasswordDialog({
  account,
  onClose,
  onSaved,
}: {
  account: Account;
  onClose: () => void;
  onSaved?: () => Promise<void>;
}) {
  const session = useIam();
  const own = session.status?.account?.id === account.id;
  const task = useIamAction();
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !task.busy) onClose();
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{own ? "修改密码" : "重置密码"}</DialogTitle>
          <DialogDescription>
            {account.displayName} 需要用新密码重新登录。
          </DialogDescription>
        </DialogHeader>
        <form
          className="space-y-4"
          onSubmit={(event) => {
            event.preventDefault();
            const data = new FormData(event.currentTarget);
            void task.run(async () => {
              await iamApi.changePassword({
                accountId: account.id,
                revision: account.revision,
                password: field(data, "password"),
                ...(own
                  ? { currentPassword: field(data, "currentPassword") }
                  : {}),
              });
              if (own) await session.logout?.();
              else await onSaved?.();
              onClose();
            });
          }}
        >
          {own && (
            <PasswordField label="当前密码" name="currentPassword" existing />
          )}
          <PasswordField label="新密码" name="password" />
          {task.error && (
            <Alert status="danger">
              <AlertDescription>{task.error}</AlertDescription>
            </Alert>
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="secondary"
              disabled={task.busy}
              onClick={onClose}
            >
              取消
            </Button>
            <Button type="submit" disabled={task.busy}>
              {task.busy ? "正在保存…" : "保存密码"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
