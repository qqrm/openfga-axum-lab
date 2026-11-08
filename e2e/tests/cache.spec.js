const { test, expect } = require("@playwright/test");
const { base, login, token, headers, ids } = require("./helpers");

test("identity change clears privileged data and rejects an in-flight old response", async ({ page }) => {
  await login(page, "alice");
  await expect.poll(() => ids(page)).toEqual(["errors", "latency", "requests"]);
  let captured;
  const intercepted = new Promise(resolve => { captured = resolve; });
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  await page.route(`**${base}?*`, async route => {
    const response = await route.fetch(); // real Axum response, held only to exercise the browser race
    captured(); await gate; await route.fulfill({ response });
  });
  await page.locator("#refresh").click();
  await intercepted;
  await page.locator("#who").selectOption("dave");
  await expect(page.locator("#workspace")).toBeHidden();
  await expect(page.locator("#create-namespace option, #new-namespace option, #role-id option")).toHaveCount(0);
  await expect(page.locator("#object-details")).toBeEmpty();
  release();
  await expect(page.locator("#error")).toContainText("discarded an earlier response");
  await expect(page.locator("#tables tr")).toHaveCount(0);
  await page.unroute(`**${base}?*`);
  await login(page, "dave");
  await expect.poll(() => ids(page)).toEqual(["latency"]);
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
