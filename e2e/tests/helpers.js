const { expect } = require("@playwright/test");
const base = "/projects/acme/warehouses/analytics/tables";
const password = process.env.DEMO_PASSWORD;
if (!password) throw new Error("DEMO_PASSWORD is required for real-stack browser tests");
async function login(page, user) {
  await page.goto("/");
  await signIn(page, user);
}
async function signIn(page, user) {
  await page.locator("#who").selectOption(user);
  await page.locator("#password").fill(password);
  await page.locator("#sign-in").click();
  await expect(page.locator("#session-state")).toHaveText(`Authenticated as ${user}`);
  await expect(page.locator("#workspace")).toBeVisible();
}
async function token(request, user) {
  const response = await request.post("/session", { data: { username: user, password } });
  expect(response.status()).toBe(200);
  return (await response.json()).token;
}
function headers(value) { return { Authorization: `Bearer ${value}` }; }
async function ids(page) { return (await page.locator("#tables td:first-child small").allTextContents()).sort(); }
async function refresh(page, expected) {
  await page.locator("#refresh").click();
  await expect.poll(() => ids(page)).toEqual([...expected].sort());
}
async function grant(request, value, method, id = "dave", privilege = "select") {
  return request.fetch(`${base}/requests/grants/${privilege}`, { method, headers: headers(value), data: { subject: { type: "user", id } } });
}
module.exports = { base, login, signIn, token, headers, ids, refresh, grant };
