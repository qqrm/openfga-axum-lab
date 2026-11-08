const { test, expect } = require("@playwright/test");
const { login, signIn, api, createTask, removeTask, refresh, selectTask, selectOrganization, expectIds } = require("../support/lab");

function expectProtectedHeaders(response) {
  const headers = response.headers();
  expect(headers["cache-control"]).toMatch(/\bno-store\b/);
  expect(headers["x-content-type-options"]).toBe("nosniff");
  expect(headers["referrer-policy"]).toBe("no-referrer");
  const directives = new Map((headers["content-security-policy"] || "").split(";").map((value) => {
    const [name, ...sources] = value.trim().split(/\s+/);
    return [name, sources];
  }));
  for (const directive of ["default-src", "base-uri", "frame-ancestors"]) {
    expect(directives.get(directive), directive).toEqual(["'none'"]);
  }
  for (const directive of ["script-src", "style-src", "connect-src", "form-action"]) {
    expect(directives.get(directive), directive).toEqual(["'self'"]);
  }
}

async function within(promise, description, milliseconds = 10000) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(description + " timed out")), milliseconds); }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

test("public provenance and authenticated responses retain no-store and browser security headers", async ({ page, request }) => {
  const document = await page.goto("/");
  expect(document.status()).toBe(200);
  expectProtectedHeaders(document);
  const provenance = await request.get("/model");
  expect(provenance.status()).toBe(200);
  expectProtectedHeaders(provenance);
  expect(await provenance.json()).toMatchObject({
    project: "Openlane",
    commit: "1c38df0a53530c8c39eb0784862b06eaa3e26f58",
    license: "Apache-2.0",
  });
  const schema = await request.get("/model/schema");
  expect(schema.status()).toBe(200);
  expectProtectedHeaders(schema);
  const model = await schema.json();
  expect(model.schema_version).toBe("1.1");
  expect(model.type_definitions.map((definition) => definition.type).sort()).toEqual(["organization", "task", "user"]);
  const [session, token] = await Promise.all([
    page.waitForResponse((response) => new URL(response.url()).pathname === "/session" && response.request().method() === "POST"),
    signIn(page, "dave"),
  ]);
  expectProtectedHeaders(session);
  expectProtectedHeaders(await api(request, "GET", "/tasks", token, undefined, 200));
  expectProtectedHeaders(await api(request, "GET", "/tasks/access-review", token, undefined, 403));
  const anonymous = await request.get("/tasks");
  expect(anonymous.status()).toBe(401);
  expectProtectedHeaders(anonymous);
});

test("identity change clears privileged data and rejects a delayed real response in the new session", async ({ page, request }) => {
  const alice = await login(page, "alice");
  await selectOrganization(page, "acme");
  await selectTask(page, "access-review");
  let captured;
  let heldRequest;
  let handlerError;
  let handlerFinished = Promise.resolve();
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  let holdNextResponse = true;
  const pattern = "**/tasks";
  const handler = (route) => {
    if (!holdNextResponse || route.request().method() !== "GET") return route.continue();
    holdNextResponse = false;
    heldRequest = route.request();
    handlerFinished = (async () => {
      // Hold an unchanged real Axum/OpenFGA response; authorization is not mocked.
      const response = await route.fetch({ timeout: 10000 });
      captured = { status: response.status(), ids: (await response.json()).map((task) => task.id).sort() };
      await gate;
      await route.fulfill({ response });
    })();
    return handlerFinished.catch((error) => { handlerError = error; });
  };
  await page.route(pattern, handler);
  let failure;
  try {
    await test.step("capture Alice's real authorized response before switching accounts", async () => {
      await page.click("#refresh");
      await expect.poll(() => {
        if (handlerError) throw handlerError;
        return captured?.status;
      }).toBe(200);
      expect(captured.ids).toEqual(["access-review", "incident-response", "vendor-review"]);
    }, { timeout: 15000 });
    const revoked = page.waitForResponse((response) => new URL(response.url()).pathname === "/session" && response.request().method() === "DELETE");
    await page.selectOption("#who", "dave");
    expect((await revoked).status()).toBe(204);
    await expect(page.locator("#workspace")).toBeHidden();
    await expect(page.locator("#tasks")).toBeEmpty();
    await expect(page.locator("#members")).toBeEmpty();
    await expect(page.locator("#create-organization option, #new-owner option")).toHaveCount(0);
    await expect(page.locator("#task-context")).toBeEmpty();
    await expect(page.locator("#edit-description")).toHaveValue("");
    await api(request, "GET", "/tasks", alice, undefined, 401);
    // Stay in the same document, so the old request can still reach its handler.
    await signIn(page, "dave");
    await expectIds(page, ["vendor-review"]);
    await test.step("deliver that same Alice response into Dave's current session", async () => {
      // headers() is provisional and may omit security headers. Correlate the
      // exact intercepted Request instead of depending on a bearer header.
      const delayed = page.waitForResponse((response) => response.request() === heldRequest, { timeout: 10000 });
      release();
      const response = await delayed;
      expect(response.status()).toBe(200);
      await within(response.finished(), "delayed response completion");
      // A foreground page avoids animation-frame throttling in headless CI.
      await page.bringToFront();
      await within(page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)))), "browser processing of the delayed response", 3000);
    }, { timeout: 15000 });
    await expect(page.locator("#session-status")).toHaveText("Сеанс: dave");
    await expectIds(page, ["vendor-review"]);
    await expect(page.locator("#task-detail")).toBeHidden();
    await expect(page.locator("#organization-management")).toBeHidden();
    await expect(page.locator("#error")).toBeEmpty();
  } catch (error) {
    failure = error;
    throw error;
  } finally {
    release();
    const cleanup = await Promise.allSettled([
      within(handlerFinished, "delayed route cleanup", 5000),
      page.isClosed() ? Promise.resolve() : within(page.unroute(pattern, handler), "route removal", 5000),
    ]);
    const cleanupErrors = cleanup.filter((result) => result.status === "rejected").map((result) => result.reason);
    if (cleanupErrors.length) {
      if (!failure) throw cleanupErrors[0];
      // Preserve the original assertion/deadline failure when teardown has
      // already closed the page; cleanup errors remain visible as annotations.
      for (const error of cleanupErrors) test.info().annotations.push({ type: "cleanup", description: String(error) });
    }
  }
});

test("untrusted task metadata stays literal in the list and detail view", async ({ page, request }) => {
  const bob = await login(page, "bob");
  const title = '<img src=x onerror="window.metadataExecuted=true">';
  const description = "<script>window.metadataExecuted=true</script>";
  let id;
  try {
    id = await createTask(request, bob, title, description);
    await refresh(page);
    await expect(page.locator('[data-task-id="' + id + '"] .task-title')).toHaveText(title);
    await selectTask(page, id);
    await expect(page.locator("#task-heading")).toHaveText(title);
    await expect(page.locator("#edit-title")).toHaveValue(title);
    await expect(page.locator("#edit-description")).toHaveValue(description);
    await expect(page.locator("#tasks img, #tasks script, #task-detail img, #task-detail script")).toHaveCount(0);
    expect(await page.evaluate(() => window.metadataExecuted)).toBeUndefined();
  } finally {
    await removeTask(request, bob, id);
  }
});
