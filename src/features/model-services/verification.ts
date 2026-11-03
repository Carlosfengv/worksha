import { providersApi } from "@/features/providers/api";
import { availableProviderModelIds } from "@/features/providers/catalog";
import type {
  Provider,
  ProviderVerification,
} from "@/features/providers/types";
import type { ModelService, ModelServiceDraft } from "./types";

export async function verifyServiceModels(
  draft: ModelServiceDraft,
  providers: Provider[],
  progress: (message: string) => void,
  previous?: ModelService | null,
  isCurrent: () => boolean = () => true,
  verify: (
    id: string,
    revision: number,
    model: string,
  ) => Promise<ProviderVerification> = providersApi.verifyProvider,
) {
  const checked = new Set<string>();
  for (const [index, model] of draft.models.entries()) {
    if (!isCurrent()) throw new Error("验证已取消，未提交模型服务。");
    const source = providers.find(
      (p) => p.id === model.providerId && p.configured,
    );
    if (!source || !availableProviderModelIds(source).includes(model.model))
      throw new Error(`${model.name || "模型"}：来源不存在或模型未启用`);
    const old = previous?.models.find((m) => m.id === model.id);
    if (
      old &&
      old.provider.id === source.id &&
      old.provider.revision === source.revision &&
      old.provider.model === model.model &&
      JSON.stringify(old.capabilities) === JSON.stringify(model.capabilities)
    )
      continue;
    const key = `${source.id}:${source.revision}:${model.model}`;
    if (checked.has(key)) continue;
    progress(
      `正在验证 ${index + 1}/${draft.models.length} · ${model.name || model.model}`,
    );
    const result = await verify(source.id, source.revision, model.model);
    if (!isCurrent()) throw new Error("验证已取消，未提交模型服务。");
    if (result.state !== "verified")
      throw new Error(`${model.name || model.model}：${result.message}`);
    checked.add(key);
  }
}
