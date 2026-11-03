import { validCredentialLabel } from "./credential-recovery-records";

export function validateCredentialInput(label: string, secret: string) {
  if (
    !validCredentialLabel(label) ||
    !secret ||
    secret.length > 8192 ||
    /[^\x21-\x7e]/.test(secret) ||
    label.includes(secret)
  )
    throw Error("请输入凭据名称和有效 API Key，名称中不能包含 API Key。");
}
