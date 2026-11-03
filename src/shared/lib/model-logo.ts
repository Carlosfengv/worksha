import claude from "@lobehub/icons-static-svg/icons/claude.svg";
import deepseek from "@lobehub/icons-static-svg/icons/deepseek.svg";
import gemini from "@lobehub/icons-static-svg/icons/gemini.svg";
import grok from "@lobehub/icons-static-svg/icons/grok.svg";
import meta from "@lobehub/icons-static-svg/icons/meta.svg";
import mistral from "@lobehub/icons-static-svg/icons/mistral.svg";
import openai from "@lobehub/icons-static-svg/icons/openai.svg";
import qwen from "@lobehub/icons-static-svg/icons/qwen.svg";

/** Model family marks are independent from the Agent client or gateway provider. */
export function modelLogo(name: string): string | undefined {
  const model = name.trim().toLowerCase().split("/").at(-1) ?? "";
  if (/^deepseek(?:[-_\s]|$)/.test(model)) return deepseek;
  if (/^(?:claude|opus|sonnet|haiku)(?:[-_\s]|$)/.test(model)) return claude;
  if (/^(?:gpt|chatgpt|codex|o[134])(?:[-_\s]|$)/.test(model)) return openai;
  if (/^gemini(?:[-_\s]|$)/.test(model)) return gemini;
  if (/^qwen(?:\d|[-_\s]|$)/.test(model)) return qwen;
  if (
    /^(?:mistral|mixtral|codestral|magistral|devstral)(?:[-_\s]|$)/.test(model)
  )
    return mistral;
  if (/^grok(?:[-_\s]|$)/.test(model)) return grok;
  if (/^llama(?:\d|[-_\s]|$)/.test(model)) return meta;
  return undefined;
}
