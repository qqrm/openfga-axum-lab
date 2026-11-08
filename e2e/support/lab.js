const { expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");

const PASSWORD = process.env.DEMO_PASSWORD;
if (!PASSWORD) {
  throw new Error("Set DEMO_PASSWORD to the password used by the running disposable lab before running Playwright.");
}

async function login(page, username) {
  await page.goto("/");
  await page.selectOption("#who", username);
  await page.fill("#password", PASSWORD);
  const [response] = await Promise.all([
    page.waitForResponse((response) => new URL(response.url()).pathname === "/session" && response.request().method() === "POST"),
    page.click("#sign-in"),
  ]);
  expect(response.status()).toBe(200);
  const { token } = await response.json();
  expect(typeof token).toBe("string");
  await expect(page.locator("#task-count")).toContainText("Доступно задач:");
  await expect(page.locator("#workspace")).toHaveJSProperty("inert", false);
  await expect(page.locator("#error")).toBeEmpty();
  return token;
}
async function apiToken(request, username) {
  const response = await request.post("/session", { data: { username, password: PASSWORD } });
  expect(response.status()).toBe(200);
  return (await response.json()).token;
}
function headers(token) { return { Authorization: "Bearer " + token }; }
async function api(request, method, path, token, data, status = 204) {
  const requestHeaders = headers(token);
  if (method === "POST" && path === "/tasks") requestHeaders["Idempotency-Key"] = randomUUID();
  const response = await request.fetch(path, { method, headers: requestHeaders, data });
  expect(response.status(), method + " " + path).toBe(status);
  return response;
}
async function createTask(request, token, title = "E2E access review") {
  const response = await request.post("/tasks", {
    headers: { ...headers(token), "Idempotency-Key": randomUUID() },
    data: { organization_id: "acme", title, description: "Disposable browser verification task.", status: "todo" },
  });
  expect(response.status()).toBe(201);
  return (await response.json()).id;
}
async function removeTask(request, token, id) {
  if (id) await api(request, "DELETE", "/tasks/" + id, token);
}
async function refresh(page) {
  const [response] = await Promise.all([
    page.waitForResponse((response) => new URL(response.url()).pathname === "/tasks" && response.request().method() === "GET"),
    page.click("#refresh"),
  ]);
  expect(response.status()).toBe(200);
  await expect(page.locator("#workspace")).toHaveJSProperty("inert", false);
  await expect(page.locator("#error")).toBeEmpty();
}
async function selectTask(page, id) {
  await page.locator('[data-task-id="' + id + '"]').click();
  await expect(page.locator("#task-context")).toContainText(id);
  await expect(page.locator("#task-detail")).toBeVisible();
  await expect(page.locator("#workspace")).toHaveJSProperty("inert", false);
}
async function selectOrganization(page, id) {
  await page.selectOption("#organization", id);
  await expect(page.locator("#organization-panel")).toBeVisible();
  await expect(page.locator("#workspace")).toHaveJSProperty("inert", false);
  await expect(page.locator("#error")).toBeEmpty();
}
async function mutate(page, click, method, path) {
  const [response] = await Promise.all([
    page.waitForResponse((response) => new URL(response.url()).pathname === path && response.request().method() === method),
    click(),
  ]);
  expect(response.status(), method + " " + path).toBe(204);
  await expect(page.locator("#workspace")).toHaveJSProperty("inert", false);
  await expect(page.locator("#error")).toBeEmpty();
}
async function setTaskRelation(page, id, relation, subject, add) {
  await page.selectOption("#relation", relation);
  await page.fill("#relation-user", subject);
  await mutate(page, () => page.click(add ? "#grant-relation" : "#revoke-relation"), add ? "PUT" : "DELETE", "/tasks/" + id + "/relations/" + relation + "/" + subject);
}
async function setOrganizationPermission(page, relation, subject, add) {
  await page.selectOption("#organization-relation", relation);
  await page.fill("#permission-user", subject);
  await mutate(page, () => page.click(add ? "#grant-permission" : "#revoke-permission"), add ? "PUT" : "DELETE", "/organizations/acme/permissions/" + relation + "/" + subject);
}
async function expectIds(page, ids) {
  await expect(page.locator("#tasks [data-task-id]")).toHaveCount(ids.length);
  const actual = await page.locator("#tasks [data-task-id]").evaluateAll((elements) => elements.map((element) => element.dataset.taskId).sort());
  expect(actual).toEqual([...ids].sort());
}
async function expectTaskEditable(page, editable) {
  // Playwright's enabled/disabled matcher applies to form controls, not the
  // fieldset itself. Check every control to verify inherited disabled state.
  for (const selector of ["#edit-title", "#edit-description", "#edit-status", "#save-task"]) {
    if (editable) await expect(page.locator(selector)).toBeEnabled();
    else await expect(page.locator(selector)).toBeDisabled();
  }
}
module.exports = { PASSWORD, login, apiToken, headers, api, createTask, removeTask, refresh, selectTask, selectOrganization, mutate, setTaskRelation, setOrganizationPermission, expectIds, expectTaskEditable };
