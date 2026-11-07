const { defineConfig } = require("@playwright/test");

module.exports = defineConfig({
  testDir: "./tests",
  workers: 1,
  fullyParallel: false,
  use: {
    baseURL: process.env.BASE_URL || "http://127.0.0.1:3000",
    headless: true,
  },
  timeout: 30000,
});