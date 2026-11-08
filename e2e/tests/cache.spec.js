const { test, expect } = require("@playwright/test");
const { login, api, createTask, removeTask, refresh, selectTask } = require("../support/lab");

test("acknowledged revoke invalidates a warm allow before the next UI refresh", async ({ page, context, request }) => {
  const owner = await login(page, "bob");
  const reader = await context.newPage();
  const dave = await login(reader, "dave");
  let id;
  try {
    id = await createTask(request, owner, "Cache revocation review");
    const relation = "/tasks/" + id + "/relations/assignee/dave";
    await api(request, "PUT", relation, owner);
    for (let index = 0; index < 2; index += 1) {
      await refresh(reader);
      await selectTask(reader, id);
      await api(request, "GET", "/tasks/" + id, dave, undefined, 200);
    }
    // No TTL sleep and no new login: the first post-acknowledgement request
    // must observe revocation with the same session and warm application cache.
    await api(request, "DELETE", relation, owner);
    await api(request, "GET", "/tasks/" + id, dave, undefined, 403);
    await refresh(reader);
    await expect(reader.locator('[data-task-id="' + id + '"]')).toHaveCount(0);
    await expect(reader.locator("#task-detail")).toBeHidden();
    await expect(reader.locator("#edit-description")).toHaveValue("");
    // An idempotent repeated revoke cannot accidentally re-grant access.
    await api(request, "DELETE", relation, owner);
    await api(request, "GET", "/tasks/" + id, dave, undefined, 403);
  } finally {
    await removeTask(request, owner, id);
    await reader.close();
  }
});

test("a warm deny is invalidated when a direct grant is acknowledged", async ({ page, context, request }) => {
  const owner = await login(page, "bob");
  const reader = await context.newPage();
  const dave = await login(reader, "dave");
  let id;
  try {
    id = await createTask(request, owner, "Cache grant review");
    for (let index = 0; index < 2; index += 1) {
      await api(request, "GET", "/tasks/" + id, dave, undefined, 403);
      await refresh(reader);
    }
    await api(request, "PUT", "/tasks/" + id + "/relations/viewer/dave", owner);
    await api(request, "GET", "/tasks/" + id, dave, undefined, 200);
    await refresh(reader);
    await selectTask(reader, id);
    await expect(reader.locator("#edit-fields")).toBeDisabled();
    await expect(reader.locator("#task-access")).toBeHidden();
  } finally {
    await removeTask(request, owner, id);
    await reader.close();
  }
});
