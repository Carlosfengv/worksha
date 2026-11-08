import { useId } from "react";
import { Input } from "@/components/ui/input";

export function CredentialFields({
  mode,
}: {
  mode: "login" | "setup" | "create";
}) {
  const id = useId();
  return (
    <>
      <div className="space-y-1.5">
        <label className="text-label font-medium" htmlFor={id + "username"}>
          登录名
        </label>
        <Input
          id={id + "username"}
          name="username"
          required
          maxLength={64}
          autoComplete="username"
          autoCapitalize="none"
          spellCheck={false}
        />
        {mode !== "login" && (
          <p className="text-label text-fg-muted">
            3 至 64 位字母、数字、点、短横线或下划线。
          </p>
        )}
      </div>
      {mode !== "login" && (
        <div className="space-y-1.5">
          <label
            className="text-label font-medium"
            htmlFor={id + "displayName"}
          >
            显示名
          </label>
          <Input
            id={id + "displayName"}
            name="displayName"
            required
            maxLength={64}
            autoComplete="off"
          />
        </div>
      )}
      <PasswordField
        label={mode === "create" ? "初始密码" : "密码"}
        name="password"
        existing={mode === "login"}
      />
    </>
  );
}

export function PasswordField({
  label,
  name,
  existing = false,
}: {
  label: string;
  name: string;
  existing?: boolean;
}) {
  const id = useId();
  return (
    <div className="space-y-1.5">
      <label className="text-label font-medium" htmlFor={id}>
        {label}
      </label>
      <Input
        id={id}
        name={name}
        type="password"
        required
        minLength={existing ? undefined : 12}
        maxLength={256}
        autoComplete={existing ? "current-password" : "new-password"}
      />
      {!existing && (
        <p className="text-label text-fg-muted">至少 12 个字符。</p>
      )}
    </div>
  );
}
