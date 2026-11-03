import { readFileSync } from "node:fs";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";

const managedFiles = [...new Set(JSON.parse(readFileSync(new URL("./.zeron/install-state.json", import.meta.url), "utf8")).installations.flatMap(entry => Object.keys(entry.fileHashes ?? {})))];

export default tseslint.config(
  {
    ignores: [
      "node_modules/**",
      "dist/**",
      "target/**",
      ".work/**",
      ".playwright-mcp/**",
      ...managedFiles,
    ],
  },
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.{ts,tsx}"],
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
      "@typescript-eslint/no-unused-vars": "error",
    },
  },
);
