import fs from "node:fs/promises";
import { randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import net from "node:net";
import path from "node:path";
import os from "node:os";

export const name = "worksha-model-bridge";
export const inject = [
  "settingsController",
  "credentialsController",
  "agentDefaultModel",
];

const NAMESPACE = "llm-pi-ai";
const LIMIT = 65536;
const ROUTE = /^local-agent-[a-f0-9]{32}$/;
const TOKEN = /^[a-f0-9]{64}$/;
const keyRef = (route) => route.toUpperCase().replaceAll("-", "_") + "_API_KEY";

function selection(ctx) {
  const value = ctx.agentDefaultModel.currentSelection();
  return value?.provider && value?.model
    ? {
        provider: value.provider,
        model: value.model,
        reasoningEffort: value.reasoningEffort ?? null,
      }
    : null;
}

const sameSelection = (a, b) =>
  a === null || b === null
    ? a === b
    : a.provider === b.provider &&
      a.model === b.model &&
      a.reasoningEffort === b.reasoningEffort;

function read(ctx, route) {
  const settings = ctx.settingsController.describe();
  const namespace = settings.namespaces.find((item) => item.ns === NAMESPACE);
  if (!settings.writable || namespace?.revision == null)
    throw Error("unavailable");
  const configured = namespace.value?.providers?.[route];
  return {
    revision: namespace.revision,
    selected: selection(ctx),
    route: configured
      ? {
          endpoint: configured.baseURL,
          models: configured.models.map((model) => model.id),
        }
      : null,
  };
}

function validProfile(profile) {
  if (
    !profile ||
    !ROUTE.test(profile.route) ||
    typeof profile.name !== "string" ||
    profile.name.length > 256 ||
    !Array.isArray(profile.models) ||
    profile.models.length === 0 ||
    profile.models.length > 64
  )
    return false;
  const binding = profile.route.slice("local-agent-".length);
  const uuid = [
    binding.slice(0, 8),
    binding.slice(8, 12),
    binding.slice(12, 16),
    binding.slice(16, 20),
    binding.slice(20),
  ].join("-");
  const endpoint =
    /^http:\/\/127\.0\.0\.1:([1-9][0-9]{0,4})\/gateway\/([a-f0-9-]+)$/.exec(
      profile.endpoint,
    );
  if (!endpoint || Number(endpoint[1]) > 65535 || endpoint[2] !== uuid)
    return false;
  const ids = new Set();
  for (const model of profile.models) {
    if (
      typeof model.id !== "string" ||
      !model.id ||
      model.id.length > 128 ||
      typeof model.name !== "string" ||
      model.name.length > 256 ||
      !Number.isSafeInteger(model.contextWindow) ||
      model.contextWindow < 1 ||
      !Number.isSafeInteger(model.maxOutputTokens) ||
      model.maxOutputTokens < 1 ||
      model.maxOutputTokens > model.contextWindow ||
      ids.has(model.id)
    )
      return false;
    ids.add(model.id);
  }
  return ids.has(profile.defaultModel);
}

function owns(status, profile) {
  return (
    status.route?.endpoint === profile.endpoint &&
    JSON.stringify(status.route.models) ===
      JSON.stringify(profile.models.map((model) => model.id))
  );
}

async function mutate(ctx, ops, revision) {
  try {
    return await ctx.settingsController.mutate(NAMESPACE, ops, revision);
  } catch (error) {
    if (error?.code === "settings/conflict") throw Error("conflict");
    throw Error("unknown");
  }
}

async function change(ctx, request, instance) {
  if (request.kind === "read") {
    if (request.route !== null && !ROUTE.test(request.route))
      throw Error("invalid");
    return read(ctx, request.route);
  }
  const profile = request.profile;
  if (!validProfile(profile)) throw Error("invalid");
  const current = read(ctx, profile.route);
  const next = {
    provider: profile.route,
    model: profile.defaultModel,
    reasoningEffort: null,
  };
  const credential = keyRef(profile.route);
  const settingPath = ["providers", profile.route];
  if (request.kind === "apply") {
    if (!TOKEN.test(request.gatewayToken)) throw Error("invalid");
    if (
      instance !== request.expectedInstance ||
      current.route ||
      current.revision !== request.expectedRevision ||
      !sameSelection(current.selected, request.before)
    )
      throw Error("conflict");
    await mutate(
      ctx,
      [
        {
          op: "set",
          path: settingPath,
          value: {
            displayName: profile.name,
            apiKeyEnv: credential,
            api: "anthropic-messages",
            baseURL: profile.endpoint,
            models: profile.models.map((model) => ({
              id: model.id,
              name: model.name,
              contextWindow: model.contextWindow,
              maxTokens: model.maxOutputTokens,
              input: ["text"],
            })),
          },
        },
      ],
      current.revision,
    );
    await ctx.credentialsController.set(credential, request.gatewayToken);
    const installed = read(ctx, profile.route);
    if (
      !owns(installed, profile) ||
      !sameSelection(installed.selected, request.before)
    )
      throw Error("unknown");
    const llm = ctx.get("llm");
    if (!llm) throw Error("unknown");
    await llm.resolveCallConfig({ provider: next.provider, model: next.model });
    await ctx.agentDefaultModel.saveSelection({
      provider: next.provider,
      model: next.model,
    });
    return read(ctx, profile.route);
  }
  if (request.kind === "restore") {
    if (!owns(current, profile)) throw Error("conflict");
    const previous = request.previous;
    if (
      !previous?.provider ||
      !previous?.model ||
      previous.provider === profile.route
    )
      throw Error("invalid");
    if (sameSelection(current.selected, next)) {
      await ctx.agentDefaultModel.saveSelection({
        provider: previous.provider,
        model: previous.model,
        ...(previous.reasoningEffort === null
          ? {}
          : { reasoningEffort: previous.reasoningEffort }),
      });
    } else if (!sameSelection(current.selected, previous)) {
      throw Error("conflict");
    }
    const restored = read(ctx, profile.route);
    if (!sameSelection(restored.selected, previous) || !owns(restored, profile))
      throw Error("unknown");
    await mutate(ctx, [{ op: "unset", path: settingPath }], restored.revision);
    await ctx.credentialsController.unset(credential);
    return read(ctx, profile.route);
  }
  throw Error("invalid");
}

export function apply(ctx, config) {
  const directory =
    config?.outputDirectory ??
    path.join(os.homedir(), ".local", "share", "local-agent", "dsh-desktop");
  if (!path.isAbsolute(directory))
    throw Error("The connection bridge needs an absolute directory");
  ctx.effect(() => {
    const socketPath = path.join(directory, "agent-connection.sock");
    const keyPath = path.join(directory, "agent-connection.key");
    const token = randomBytes(32).toString("hex");
    const instance = randomUUID();
    let busy = false;
    let closed = false;
    const sockets = new Set();
    const server = net.createServer((socket) => {
      sockets.add(socket);
      socket.once("close", () => sockets.delete(socket));
      socket.setTimeout(15000, () => socket.destroy());
      socket.on("error", () => socket.destroy());
      let bytes = Buffer.alloc(0);
      let received = false;
      const reply = (payload) =>
        socket.end(JSON.stringify({ version: 1, ...payload }) + "\n");
      socket.on("data", (chunk) => {
        if (received || closed || bytes.length + chunk.length > LIMIT)
          return socket.destroy();
        bytes = Buffer.concat([bytes, chunk]);
        if (!bytes.includes(10)) return;
        received = true;
        void (async () => {
          let frame;
          try {
            frame = JSON.parse(bytes.toString("utf8"));
          } catch {
            return reply({ error: "invalid" });
          }
          if (
            frame?.version !== 1 ||
            typeof frame.token !== "string" ||
            !TOKEN.test(frame.token) ||
            !timingSafeEqual(Buffer.from(frame.token), Buffer.from(token))
          )
            return reply({ error: "denied" });
          if (busy) return reply({ error: "busy" });
          busy = true;
          try {
            reply({
              status: {
                ...(await change(ctx, frame.request ?? {}, instance)),
                instance,
              },
            });
          } catch (error) {
            const expected = ["conflict", "invalid", "unavailable"].includes(
              error?.message,
            );
            reply({ error: expected ? error.message : "unknown" });
          } finally {
            busy = false;
          }
        })();
      });
    });
    server.maxConnections = 4;
    server.on("error", () =>
      ctx.logger.warn("Local Agent model bridge could not start"),
    );
    void (async () => {
      await fs.mkdir(directory, { recursive: true, mode: 0o700 });
      const stat = await fs.lstat(directory);
      if (
        !stat.isDirectory() ||
        stat.uid !== process.getuid() ||
        stat.mode & 0o077 ||
        closed
      )
        return;
      server.listen(socketPath, () => {
        void (async () => {
          await fs.chmod(socketPath, 0o600);
          const temp = keyPath + "." + randomBytes(8).toString("hex");
          await fs.writeFile(temp, token, { mode: 0o600, flag: "wx" });
          try {
            if (!closed) await fs.rename(temp, keyPath);
          } finally {
            await fs.unlink(temp).catch(() => {});
          }
        })().catch(() => server.close());
      });
    })().catch(() =>
      ctx.logger.warn("Local Agent model bridge directory is unavailable"),
    );
    return () => {
      closed = true;
      for (const socket of sockets) socket.destroy();
      server.close();
    };
  }, "Local Agent model connection");
}
