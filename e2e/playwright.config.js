const { defineConfig } = require("@playwright/test");

module.exports = defineConfig({
  testDir: "./tests",
  workers: 1,
  fullyParallel: false,
  use: {
    baseURL: process.env.BASE_URL || "http://127.0.0.1:3000",
    headless: true,
    screenshot: "only-on-failure",
    trace: "off",
  },
  timeout: 60000,
  expect: { timeout: 10000 },
  reporter: process.argv.includes("--list") ? [["list"]] : [
    ["list"],
    ["junit", { outputFile: "../artifacts/browser-e2e-" + (process.env.FGA_TRANSPORT || "default") + ".xml" }],
  ],
});
