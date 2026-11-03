import { defineConfig } from "@playwright/test";

const port = Number(process.env.LOCAL_AGENT_TEST_PORT ?? 1425);

export default defineConfig({
  testDir: "./tests/frontend",
  outputDir: ".work/test-results",
  fullyParallel: true,
  workers: 2,
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    browserName: "chromium",
    trace: "retain-on-failure",
  },
  webServer: {
    env: { LOCAL_AGENT_TEST_PORT: String(port) },
    command: `pnpm exec vite --host 127.0.0.1 --port ${port} --strictPort`,
    url: `http://127.0.0.1:${port}/tests/frontend/`,
    reuseExistingServer: false,
  },
});
