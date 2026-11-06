import { useEffect, useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { ProjectPicker } from "@/features/projects/project-picker";
import { useIcon } from "@/lib/icon-context";
import { DesktopError } from "@/shared/lib/desktop-command";
import { workbenchApi } from "./api";
import { handoffText } from "./desktop-handoff";
import { agentLabels } from "./types";
import type { Agent, ModelRoutes, Task, TaskDraft } from "./types";

type SetupProps = {
  projectId?: string;
  source?: Task;
  onCreated: (id: string, draft?: TaskDraft) => void;
  onCancel?: () => void;
};

export function TaskSetup(props: SetupProps) {
  const [projectId, setProjectId] = useState<string | null>(
    props.projectId ?? null,
  );
  const [agent, setAgent] = useState<Agent>(
    props.source?.agent === "opencode" ? "claude" : "opencode",
  );
  const [message, setMessage] = useState(() =>
    props.source ? handoffText(props.source) : "",
  );
  const [busy, setBusy] = useState(false);
  const text = message.trim();
  const messageValid =
    Boolean(text) && new TextEncoder().encode(text).length <= 24 * 1024;
  return (
    <section className="min-w-0 space-y-5 rounded-xl border border-border bg-surface-raised p-5">
      <div className="space-y-1">
        <h2 className="text-title font-medium">
          {props.source ? "交接任务" : "新建任务"}
        </h2>
        <p className="text-body text-fg-muted">
          {props.source
            ? "确认要交给下一位 Agent 的内容。接手会话会保留原任务链接。"
            : "写下任务，再选择 Agent 和项目。"}
        </p>
      </div>
      <Field>
        <FieldLabel>
          {props.source ? "交接内容" : "想让 Agent 做什么？"}
        </FieldLabel>
        <Textarea
          rows={props.source ? 7 : 4}
          value={message}
          disabled={busy}
          onChange={(event) => setMessage(event.target.value)}
          placeholder="例如，分析测试失败的原因"
        />
        {props.source && (
          <FieldDescription>
            交接内容会放进接手会话的输入框，由你发送。
          </FieldDescription>
        )}
        {text && !messageValid && (
          <FieldDescription>单条消息最多 24 KB。</FieldDescription>
        )}
      </Field>
      <Field>
        <FieldLabel>执行 Agent</FieldLabel>
        <Select
          value={agent}
          disabled={busy || Boolean(props.source)}
          onValueChange={(value) => {
            if (value === "opencode" || value === "claude") setAgent(value);
          }}
        >
          <SelectTrigger aria-label="执行 Agent" />
          <SelectContent>
            <SelectItem value="opencode">OpenCode</SelectItem>
            <SelectItem value="claude">Claude Code CLI</SelectItem>
          </SelectContent>
        </Select>
      </Field>
      {!props.source && (
        <Field>
          <FieldLabel>项目</FieldLabel>
          <ProjectPicker
            value={projectId}
            onChange={setProjectId}
            disabled={busy}
          />
        </Field>
      )}
      <SetupRoute
        key={agent}
        {...props}
        projectId={projectId ?? undefined}
        agent={agent}
        text={text}
        messageValid={messageValid}
        busy={busy}
        setBusy={setBusy}
      />
    </section>
  );
}

function SetupRoute({
  projectId,
  source,
  onCreated,
  onCancel,
  agent,
  text,
  messageValid,
  busy,
  setBusy,
}: SetupProps & {
  agent: Agent;
  text: string;
  messageValid: boolean;
  busy: boolean;
  setBusy: (busy: boolean) => void;
}) {
  const [config, setConfig] = useState<ModelRoutes>();
  const [choice, setChoice] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const request = useRef<string | null>(null);
  const sharedCreation = useRef<{
    id: string;
    model: string;
    text: string;
    projectId: string | null;
  } | null>(null);
  const shared = agent === "opencode" && !source;
  const Folder = useIcon("folder");

  useEffect(() => {
    let mounted = true;
    void (
      shared ? workbenchApi.sharedRoutes() : workbenchApi.routes(agent)
    ).then(
      (result) => {
        if (mounted) {
          setConfig(result);
          setError(null);
        }
      },
      (reason: unknown) => {
        if (mounted) setError(String(reason));
      },
    );
    return () => {
      mounted = false;
    };
  }, [agent, shared, refresh]);
  const routes = config?.routes ?? [];
  const model = choice ?? routes.find((entry) => entry.isDefault)?.id ?? null;
  const requestScope = routes.find((entry) => entry.id === model)?.requestScope;
  const modelLabel = (id: string) => {
    const entry = routes.find((route) => route.id === id);
    return entry ? `${entry.label}${entry.isDefault ? " · 当前选用" : ""}` : id;
  };

  async function create() {
    if (!model || !messageValid || request.current) return;
    const id = crypto.randomUUID();
    const currentHash = window.location.hash;
    request.current = id;
    setBusy(true);
    setError(null);
    if (shared) {
      if (
        !sharedCreation.current ||
        sharedCreation.current.model !== model ||
        sharedCreation.current.text !== text ||
        sharedCreation.current.projectId !== (projectId ?? null)
      )
        sharedCreation.current = {
          id,
          model,
          text,
          projectId: projectId ?? null,
        };
      try {
        const created = await workbenchApi.createShared(
          sharedCreation.current.id,
          projectId ?? null,
          model,
          text,
        );
        if (!created) return;
        if (!created.delivered)
          throw new Error(
            created.error ??
              "共享会话已创建，首条消息结果待核对。可以从看板打开原会话检查。",
          );
        await workbenchApi.controlLive({
          kind: "select",
          connectionId: created.connectionId,
          sessionId: created.sessionId,
        });
        if (window.location.hash === currentHash)
          window.location.hash = `/tasks/${encodeURIComponent(created.sessionId)}?tab=live`;
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : String(reason));
      } finally {
        request.current = null;
        setBusy(false);
      }
      return;
    }
    let task: Task | null = null;
    try {
      task = await workbenchApi.create(
        id,
        projectId ?? null,
        model,
        agent,
        source?.id ?? null,
      );
    } catch (reason) {
      if (reason instanceof DesktopError && reason.outcomeUnknown) {
        try {
          task = (await workbenchApi.read(id, null)).task;
        } catch {
          // A recovered record remains visible from the board.
        }
      }
      if (!task)
        setError(reason instanceof Error ? reason.message : String(reason));
    }

    try {
      if (!task || currentHash !== window.location.hash) return;
      if (source) {
        onCreated(task.id, { text });
        return;
      }
      const turnId = crypto.randomUUID();
      try {
        await workbenchApi.send(task.id, turnId, text);
        if (window.location.hash === currentHash) onCreated(task.id);
      } catch (reason) {
        if (window.location.hash !== currentHash) return;
        onCreated(task.id, {
          text,
          error: reason instanceof Error ? reason.message : String(reason),
          uncertainTurn:
            reason instanceof DesktopError && reason.outcomeUnknown
              ? turnId
              : undefined,
        });
      }
    } finally {
      request.current = null;
      setBusy(false);
    }
  }

  const modelPicker = (
    <Field>
      <FieldLabel>模型</FieldLabel>
      <Combobox<string>
        items={routes.map((route) => route.id)}
        value={model}
        onValueChange={(id) => setChoice(id ?? "")}
        itemToStringLabel={modelLabel}
        disabled={busy || !routes.length}
      >
        <ComboboxInput
          aria-label="模型"
          placeholder={config ? "选择已配置的模型" : "正在读取 Agent 配置"}
          triggerAriaLabel="展开模型"
        />
        <ComboboxContent>
          <ComboboxEmpty>没有匹配的模型</ComboboxEmpty>
          <ComboboxList>
            {(id: string) => (
              <ComboboxItem key={id} value={id}>
                {modelLabel(id)}
              </ComboboxItem>
            )}
          </ComboboxList>
        </ComboboxContent>
      </Combobox>
    </Field>
  );

  return (
    <>
      {shared && (
        <p className="text-label text-fg-muted">
          任务保存在 OpenCode 的默认本机服务中，也可以在 OpenCode Desktop
          查看和继续。
        </p>
      )}
      {requestScope === "other" && (
        <Alert status="warning">
          <AlertTitle>模型连接了另一个 App 网关</AlertTitle>
          <AlertDescription>
            请在 Agents 中将 {agentLabels[agent]} 连接到当前 App
            后重试。任务尚未发送，所选模型保持不变。
          </AlertDescription>
        </Alert>
      )}
      {requestScope === "external" && (
        <p className="text-label text-fg-muted">
          此模型直连外部服务，请求不会记录到当前 App 的网关中。
        </p>
      )}
      {requestScope === "unknown" && (
        <p role="status" className="text-label text-fg-muted">
          模型入口的网关归属暂时无法确认，请核对连接后重试。
        </p>
      )}
      {model ? (
        <details className="space-y-3">
          <summary className="cursor-pointer text-label text-fg-muted">
            模型 · {modelLabel(model)}
          </summary>
          {modelPicker}
        </details>
      ) : (
        modelPicker
      )}
      {config && (!config.installed || config.error || !routes.length) && (
        <Alert status="warning">
          <AlertTitle>{agentLabels[agent]} 尚未就绪</AlertTitle>
          <AlertDescription>
            {!config.installed
              ? `请先安装 ${agentLabels[agent]}。`
              : (config.error ?? "请先在 Agent 的用户配置中设置模型。")}
          </AlertDescription>
        </Alert>
      )}
      {error && (
        <Alert status="danger">
          <AlertTitle>未能创建任务</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <Button
          leadingIcon={Folder}
          disabled={
            busy ||
            !config?.installed ||
            !messageValid ||
            requestScope === "other" ||
            requestScope === "unknown" ||
            !routes.some((route) => route.id === model)
          }
          onClick={() => void create()}
        >
          {busy
            ? "正在确认项目…"
            : source
              ? "确认项目目录"
              : projectId
                ? "确认项目并发送"
                : "选择文件夹并发送"}
        </Button>
        {(error || config?.error || !model) && (
          <Button
            variant="tertiary"
            disabled={busy}
            onClick={() => setRefresh((value) => value + 1)}
          >
            重新读取配置
          </Button>
        )}
        {onCancel && (
          <Button variant="ghost" disabled={busy} onClick={onCancel}>
            返回原任务
          </Button>
        )}
      </div>
    </>
  );
}
