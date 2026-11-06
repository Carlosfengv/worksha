import { useState, type ComponentProps } from "react";
import { createRoot } from "react-dom/client";
import { SessionThread } from "@/features/workbench/session-thread";
import { IconProvider } from "@/lib/icon-context";
import "@/styles/globals.css";

function Harness() {
  const [parts, setParts] = useState<
    ComponentProps<typeof SessionThread>["parts"]
  >([
    { id: "user-1", role: "user", text: "历史问题" },
    { id: "assistant-1", role: "assistant", text: "历史回复" },
  ]);
  const [streaming, setStreaming] = useState(false);
  const [draft, setDraft] = useState("");
  return (
    <main className="flex h-dvh flex-col gap-4 p-4">
      <div>
        <button onClick={() => setParts([])}>清空消息</button>
        <button
          onClick={() =>
            setParts([{ id: "tool-only", role: "tool", text: "已停止本轮" }])
          }
        >
          仅工具记录
        </button>
        <button
          onClick={() => {
            setStreaming(true);
            setParts((current) => [
              ...current,
              { id: "user-2", role: "user", text: "新问题" },
              { id: "assistant-2", role: "assistant", text: "首段回复" },
            ]);
          }}
        >
          追加一轮
        </button>
        <button
          onClick={() => {
            setParts((current) =>
              current.map((part) =>
                part.id === "assistant-2"
                  ? { ...part, text: "首段回复，追加内容" }
                  : part,
              ),
            );
            setStreaming(false);
          }}
        >
          完成本轮
        </button>
      </div>
      <SessionThread
        parts={parts}
        agent="OpenCode"
        busy={streaming}
        streaming={streaming}
        composer={
          <textarea
            aria-label="草稿"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
        }
      />
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <IconProvider>
    <Harness />
  </IconProvider>,
);
