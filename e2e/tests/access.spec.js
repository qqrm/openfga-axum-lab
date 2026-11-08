const { test, expect } = require("@playwright/test");
const { randomUUID } = require("node:crypto");
const { login, api, createTask, removeTask, refresh, selectTask, mutate, setTaskRelation, expectIds, expectTaskEditable } = require("../support/lab");

// These tests use the real application's HTTP routes and its configured
// PostgreSQL/OpenFGA backend. No network routes or authorization are mocked.
for (const [username, ids] of [
  ["alice", ["access-review", "vendor-review", "incident-response"]],
  ["carol", ["access-review", "incident-response"]],
  ["dave", ["vendor-review"]],
  ["erin", ["other-review"]],
]) {
  test(username + " sees only authorized seeded tasks", async ({ page }) => {
    await login(page, username);
    await expectIds(page, ids);
  });
}

test("assignee can edit but cannot delete or grant rights", async ({ page, request }) => {
  const token = await login(page, "carol");
  await selectTask(page, "incident-response");
  await expectTaskEditable(page, true);
  await expect(page.locator("#delete-task")).toBeHidden();
  await expect(page.locator("#task-access")).toBeHidden();
  await api(request, "DELETE", "/tasks/incident-response", token, undefined, 403);
  await api(request, "PUT", "/tasks/incident-response/relations/editor/dave", token, undefined, 403);
  await api(request, "PUT", "/organizations/acme/members/dave", token, { role: "admin" }, 403);
  await api(request, "GET", "/organizations/other/members", token, undefined, 403);
});

test("external task viewer sees no editing controls", async ({ page, request }) => {
  const token = await login(page, "dave");
  await selectTask(page, "vendor-review");
  await expectTaskEditable(page, false);
  await expect(page.locator("#delete-task")).toBeHidden();
  await expect(page.locator("#task-access")).toBeHidden();
  await api(request, "GET", "/tasks/access-review", token, undefined, 403);
});

test("invalid password gives an explicit login error", async ({ page }) => {
  await page.goto("/");
  await page.fill("#password", "invalid-password-for-negative-control");
  await page.click("#sign-in");
  await expect(page.locator("#error")).toContainText("Не удалось войти");
  await expect(page.locator("#workspace")).toBeHidden();
});

test("switching accounts immediately clears the previous task and session", async ({ page, request }) => {
  const token = await login(page, "alice");
  await selectTask(page, "access-review");
  const revoked = page.waitForResponse((response) => new URL(response.url()).pathname === "/session" && response.request().method() === "DELETE");
  await page.selectOption("#who", "dave");
  expect((await revoked).status()).toBe(204);
  await expect(page.locator("#workspace")).toBeHidden();
  await expect(page.locator("#tasks")).toBeEmpty();
  await expect(page.locator("#edit-description")).toHaveValue("");
  await api(request, "GET", "/tasks", token, undefined, 401);
});

test("create, edit, reload and delete a task through the actual UI", async ({ page, request }) => {
  const token = await login(page, "bob");
  let id;
  try {
    await page.click("#new-task");
    await page.selectOption("#create-organization", "acme");
    await page.fill("#create-title", "E2E review " + randomUUID().slice(0, 8));
    await page.fill("#create-description", '<img src="x" onerror="window.untrustedTaskExecuted=true">');
    const [created] = await Promise.all([
      page.waitForResponse((response) => new URL(response.url()).pathname === "/tasks" && response.request().method() === "POST"),
      page.click("#create-submit"),
    ]);
    expect(created.status()).toBe(201);
    id = (await created.json()).id;
    await expect(page.locator("#task-context")).toContainText(id);
    await expect(page.locator("#task-context")).toContainText("assigner: bob");
    await expect(page.locator("#workspace")).toHaveJSProperty("inert", false);
    expect(await page.evaluate(() => window.untrustedTaskExecuted)).toBeUndefined();
    await page.fill("#edit-description", "Evidence verified in the browser.");
    await page.selectOption("#edit-status", "done");
    await mutate(page, () => page.click("#save-task"), "PATCH", "/tasks/" + id);
    await refresh(page);
    await expect(page.locator("#edit-description")).toHaveValue("Evidence verified in the browser.");
    await expect(page.locator("#edit-status")).toHaveValue("done");
    const persisted = await api(request, "GET", "/tasks/" + id, token, undefined, 200);
    expect((await persisted.json()).status).toBe("done");
    page.once("dialog", (dialog) => dialog.accept());
    await mutate(page, () => page.click("#delete-task"), "DELETE", "/tasks/" + id);
    await expect(page.locator('[data-task-id="' + id + '"]')).toHaveCount(0);
    await api(request, "GET", "/tasks/" + id, token, undefined, 404);
  } finally {
    await removeTask(request, token, id);
  }
});

test("grant, block, unblock and revoke task access with independent paths", async ({ page, context, request }) => {
  const managerToken = await login(page, "bob");
  const reader = await context.newPage();
  const readerToken = await login(reader, "dave");
  let id;
  try {
    id = await createTask(request, managerToken);
    await refresh(page);
    await selectTask(page, id);
    await setTaskRelation(page, id, "viewer", "dave", true);
    await refresh(reader);
    await selectTask(reader, id);
    await expectTaskEditable(reader, false);
    await setTaskRelation(page, id, "blocked", "dave", true);
    await refresh(reader);
    await expect(reader.locator('[data-task-id="' + id + '"]')).toHaveCount(0);
    await api(request, "GET", "/tasks/" + id, readerToken, undefined, 403);
    await api(request, "PATCH", "/tasks/" + id, readerToken, { title: "Denied", description: "", status: "todo" }, 403);
    await setTaskRelation(page, id, "blocked", "dave", false);
    await refresh(reader);
    await selectTask(reader, id);
    await setTaskRelation(page, id, "assignee", "dave", true);
    await refresh(reader);
    await expectTaskEditable(reader, true);
    await expect(reader.locator("#delete-task")).toBeHidden();
    await expect(reader.locator("#task-access")).toBeHidden();
    await api(request, "PUT", "/tasks/" + id + "/relations/viewer/erin", readerToken, undefined, 403);
    await api(request, "DELETE", "/tasks/" + id, readerToken, undefined, 403);
    await reader.fill("#edit-description", "Completed by the assignee.");
    await mutate(reader, () => reader.click("#save-task"), "PATCH", "/tasks/" + id);
    await setTaskRelation(page, id, "viewer", "dave", false);
    await refresh(reader);
    await expect(reader.locator('[data-task-id="' + id + '"]')).toHaveCount(1);
    await setTaskRelation(page, id, "assignee", "dave", false);
    await refresh(reader);
    await expect(reader.locator('[data-task-id="' + id + '"]')).toHaveCount(0);
    await expect(reader.locator("#task-detail")).toBeHidden();
    await api(request, "GET", "/tasks/" + id, readerToken, undefined, 403);
  } finally {
    await removeTask(request, managerToken, id);
    await reader.close();
  }
});
