import type { ReactNode } from "react";
import { AnimatePresence, MotionConfig } from "framer-motion";
import {
  ChatAssistantMessage,
  ChatThread,
  ChatUserMessage,
  chatThreadZhLabels,
} from "@/components/ui/chat-thread";
import { Markdown, type MarkdownComponents } from "@/components/ui/markdown";

type SessionPart = {
  id: string;
  role: "user" | "assistant" | "tool";
  text: string;
  truncated?: boolean;
};

const messageComponents: MarkdownComponents = {
  a: ({ children }) => <span>{children}</span>,
  img: ({ alt }) => (alt ? <span>{alt}</span> : null),
};

export function SessionThread({
  parts,
  agent,
  busy,
  streaming,
  composer,
}: {
  parts: SessionPart[];
  agent: string;
  busy: boolean;
  streaming: boolean;
  composer: ReactNode;
}) {
  const roles = parts.map((part) => part.role);
  const lastUser = roles.lastIndexOf("user");
  const lastAssistant = roles.lastIndexOf("assistant");
  const onlyTools =
    parts.length > 0 && parts.every((part) => part.role === "tool");
  return (
    <MotionConfig reducedMotion="always">
      <div className="min-h-0 flex-1">
        <ChatThread
          locale="zh-CN"
          labels={chatThreadZhLabels}
          busy={busy}
          empty={
            !parts.length ? (
              <p className="text-body text-fg-muted">
                告诉 {agent} 你想完成的任务。
              </p>
            ) : onlyTools ? (
              <div className="space-y-2">
                {parts.map((part) => (
                  <p
                    key={part.id}
                    className="break-words text-label text-fg-muted"
                  >
                    {part.text}
                  </p>
                ))}
              </div>
            ) : (
              <></>
            )
          }
          composer={<div className="space-y-3">{composer}</div>}
        >
          {parts.map((part, index) => {
            // Paint conversation text immediately, including when native animation
            // frames are paused while the window is hidden or being restored.
            if (part.role === "tool")
              return (
                <p
                  key={part.id}
                  className="break-words text-label text-fg-muted"
                >
                  {part.text}
                </p>
              );
            if (part.role === "user")
              return (
                <AnimatePresence key={part.id} initial={false}>
                  <ChatUserMessage id={part.id} text={part.text} />
                </AnimatePresence>
              );
            const last = index === lastAssistant;
            const generating = streaming && last && index > lastUser;
            return (
              <AnimatePresence key={part.id} initial={false}>
                <ChatAssistantMessage
                  id={part.id}
                  model={agent}
                  text={part.text}
                  last={last}
                  streaming={generating}
                >
                  <Markdown
                    text={part.text}
                    components={messageComponents}
                    streaming={generating}
                    streamingLabel={chatThreadZhLabels.streaming}
                  />
                  {part.truncated && (
                    <p className="text-label text-fg-subtle">
                      已达到单段显示上限
                    </p>
                  )}
                </ChatAssistantMessage>
              </AnimatePresence>
            );
          })}
        </ChatThread>
      </div>
    </MotionConfig>
  );
}
