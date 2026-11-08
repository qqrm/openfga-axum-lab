const { test, expect } = require("@playwright/test");
const { base, login, signIn, token, headers, ids } = require("./helpers");

test("identity switch revokes the old session and ignores its delayed real response", async ({ page, request }) => {
  await login(page, "alice");
  await expect.poll(() => ids(page)).toEqual(["errors", "latency", "requests"]);
  let captured;
  let release;
  let handlerError;
  let handlerFinished = Promise.resolve();
  const gate = new Promise(resolve => { release = resolve; });
  let holdNext = true;
  const pattern = `**${base}?*`;
  const handler = route => {
    if (!holdNext) return route.continue();
    holdNext = false;
    handlerFinished = (async () => {
      const response = await route.fetch({ timeout: 10000 });
      captured = { status: response.status(), ids: (await response.json()).items.map(table => table.id).sort() };
      await gate;
      await route.fulfill({ response });
    })();
    return handlerFinished.catch(error => { handlerError = error; });
  };
  await page.route(pattern, handler);
  try {
    await page.evaluate(() => {
      const original = window.loadPage;
      let finish;
      const observer = { settled: false, completion: new Promise(resolve => { finish = resolve; }) };
      window.__heldCatalogRefresh = observer;
      window.loadPage = function (...args) {
        window.loadPage = original;
        const pending = Reflect.apply(original, this, args);
        const settled = () => { observer.settled = true; finish(); };
        pending.then(settled, settled);
        return pending;
      };
    });
    await page.locator("#refresh").click();
    await expect.poll(() => {
      if (handlerError) throw handlerError;
      return captured;
    }).toEqual({ status: 200, ids: ["errors", "latency", "requests"] });
    const revoked = page.waitForResponse(response =>
      new URL(response.url()).pathname === "/session" && response.request().method() === "DELETE");
    await page.locator("#who").selectOption("dave");
    const logout = await revoked;
    expect(logout.status()).toBe(204);
    const previousToken = (await logout.request().allHeaders()).authorization.slice("Bearer ".length);
    await expect(page.locator("#workspace")).toBeHidden();
    await expect(page.locator("#create-namespace option, #new-namespace option, #role-id option")).toHaveCount(0);
    await expect(page.locator("#object-details")).toBeEmpty();
    expect((await request.get(base, { headers: headers(previousToken) })).status()).toBe(401);
    await signIn(page, "dave");
    await expect.poll(() => ids(page)).toEqual(["latency"]);
    expect(await page.evaluate(() => window.__heldCatalogRefresh.settled)).toBe(false);
    release();
    await handlerFinished;
    // The UI rejects an old response before reading its body. Await its real
    // refresh operation, not Response.finished(), which need not receive EOF.
    await page.evaluate(() => window.__heldCatalogRefresh.completion);
    if (handlerError) throw handlerError;
    await expect(page.locator("#session-state")).toHaveText("Authenticated as dave");
    await expect.poll(() => ids(page)).toEqual(["latency"]);
    await expect(page.locator("#object-details")).toBeEmpty();
    await expect(page.locator("#error")).toBeEmpty();
  } finally {
    release();
    await handlerFinished.catch(() => {});
    await page.unroute(pattern, handler);
  }
});

test("untrusted metadata renders as text and cannot execute script", async ({ page, request }) => {
  const alice = await token(request, "alice");
  const name = '<img src=x onerror="window.metadataExecuted=true">';
  const created = await request.post(base, { headers: { ...headers(alice), "Idempotency-Key": crypto.randomUUID() }, data: { namespace_id: "metrics", name, description: "<script>window.metadataExecuted=true</script>" } });
  expect(created.status()).toBe(201);
  const id = (await created.json()).id;
  try {
    await login(page, "alice");
    await expect(page.locator("#tables")).toContainText(name);
    expect(await page.evaluate(() => window.metadataExecuted)).toBeUndefined();
    await expect(page.locator("#tables img, #tables script")).toHaveCount(0);
  } finally { expect((await request.delete(`${base}/${id}`, { headers: headers(alice) })).status()).toBe(204); }
});
