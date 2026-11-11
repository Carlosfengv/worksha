import { availableProviderModelIds } from "@/features/providers/catalog";
import type {
  Provider,
  ProviderVerification,
  UpstreamAccountRef,
} from "@/features/providers/types";
import type { ModelService, ModelServiceDraft } from "./types";

export async function verifyServiceModels(
  draft: ModelServiceDraft,
  providers: Provider[],
  progress: (message: string) => void,
  verify: (
    id: string,
    revision: number,
    model: string,
    account?: UpstreamAccountRef,
  ) => Promise<ProviderVerification>,
  previous?: ModelService | null,
  isCurrent: () => boolean = () => true,
) {
  const checked = new Set<string>();
  for (const [index, model] of draft.models.entries()) {
    if (!isCurrent()) throw new Error("验证已取消，未提交模型服务。");
    if (model.child) continue;
    const source = providers.find(
      (p) => p.id === model.providerId && p.configured,
    );
    if (!source || !availableProviderModelIds(source).includes(model.model))
      throw new Error(`${model.name || "模型"}：来源不存在或模型未启用`);
    const old = previous?.models.find((m) => m.id === model.id);
    const unchanged =
      old &&
      !old.child &&
      old.provider.id === source.id &&
      old.provider.revision === source.revision &&
      old.provider.model === model.model &&
      JSON.stringify(old.capabilities) === JSON.stringify(model.capabilities);
    const pool = draft.routingPolicy?.accountPools?.find(
      (p) => p.modelId === model.id,
    );
    const previousPool = previous?.routingPolicy?.accountPools?.find(
      (p) => p.modelId === model.id,
    );
    const accounts = pool?.accounts.length ? pool.accounts : [undefined];
    for (const [accountIndex, account] of accounts.entries()) {
      if (
        unchanged &&
        (account
          ? previousPool?.accounts.some(
              (a) => a.id === account.id && a.revision === account.revision,
            )
          : !previousPool?.accounts.length)
      )
        continue;
      if (!isCurrent()) throw new Error("验证已取消，未提交模型服务。");
      const key = JSON.stringify([
        source.id,
        source.revision,
        model.model,
        account,
      ]);
      if (checked.has(key)) continue;
      progress(
        `正在验证 ${index + 1}/${draft.models.length} · ${model.name || model.model}${account ? ` · 账号 ${accountIndex + 1}/${accounts.length}` : ""}`,
      );
      const result = await verify(
        source.id,
        source.revision,
        model.model,
        account,
      );
      if (!isCurrent()) throw new Error("验证已取消，未提交模型服务。");
      if (result.state !== "verified")
        throw new Error(`${model.name || model.model}：${result.message}`);
      checked.add(key);
    }
  }
}
