import { Button } from "@/components/ui/button";
import { documentationCurrent } from "../model-metadata";
import type { ModelDocumentation } from "../types";

export function ProviderModelEvidence({
  sources,
}: {
  sources: ModelDocumentation[];
}) {
  if (!sources.length) return null;
  const current = sources.every((source) => documentationCurrent(source));
  return (
    <div className="mt-2 flex max-w-64 flex-col items-start gap-1 whitespace-normal">
      <p>
        {current ? "官方资料补充" : "官方资料已过期"} · 核对日期{" "}
        {sources[0].checkedAt.slice(0, 10)}
      </p>
      {sources.map((source) => (
        <Button key={source.url} variant="link" size="sm" asChild>
          <a href={source.url} target="_blank" rel="noreferrer">
            {source.label}
          </a>
        </Button>
      ))}
    </div>
  );
}
