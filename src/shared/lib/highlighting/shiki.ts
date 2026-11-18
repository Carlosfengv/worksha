import {
  createBundledHighlighter,
  createSingletonShorthands,
  guessEmbeddedLanguages,
} from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import { bundledThemes } from "shiki/themes";

export * from "shiki/core";
export { createJavaScriptRegexEngine } from "shiki/engine/javascript";
export { createOnigurumaEngine } from "shiki/engine/oniguruma";

// The desktop artifact viewer highlights the JSON configuration summary.
export const bundledLanguages = { json: () => import("@shikijs/langs/json") };
export const createHighlighter = createBundledHighlighter({
  langs: bundledLanguages,
  themes: bundledThemes,
  engine: createJavaScriptRegexEngine,
});
export const { codeToHtml } = createSingletonShorthands(createHighlighter, {
  guessEmbeddedLanguages,
});
