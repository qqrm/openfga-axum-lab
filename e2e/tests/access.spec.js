const { test, expect } = require("@playwright/test");
const { base, login, token, headers, ids, refresh, grant } = require("./helpers");

test("public model provenance and session-bound filtered catalog", async ({ page }) => {
  for (const [user, expected] of [["alice", ["errors", "latency", "requests"]], ["bob", ["errors", "latency", "requests"]], ["carol", ["requests"]], ["dave", ["latency"]], ["eve", ["requests"]]]) {
    await login(page, user);
    await expect.poll(() => ids(page)).toEqual(expected.sort());
    await expect(page.locator("#model-name")).toContainText("Lakekeeper model v4.15");
    await expect(page.locator("#scope option")).toContainText(user === "eve" ? "Other" : "Acme");
  }
  await page.locator("#sign-out").click();
  await expect(page.locator("#workspace")).toBeHidden();
  await expect(page.locator("#tables tr")).toHaveCount(0);
});

test("real grant and revoke update the browser; reader cannot escalate", async ({ page, request }) => {
  const carol = await token(request, "carol"), dave = await token(request, "dave");
  expect((await grant(request, carol, "DELETE")).status()).toBe(204);
  await login(page, "dave");
  await expect.poll(() => ids(page)).toEqual(["latency"]);
  expect((await grant(request, dave, "PUT", "dave", "manage_grants")).status()).toBe(403);
  try {
    expect((await grant(request, carol, "PUT")).status()).toBe(204);
    await refresh(page, ["latency", "requests"]);
    await page.locator("#object-id").fill("requests");
    await page.locator("#action").selectOption("can_read_data");
    await page.locator("#check-form button").click();
    await expect(page.locator("#decision")).toHaveText("ALLOW · can_read_data");
    expect((await grant(request, carol, "DELETE")).status()).toBe(204);
    await refresh(page, ["latency"]);
    await page.locator("#check-form button").click();
    await expect(page.locator("#decision")).toHaveText("DENY · can_read_data");
  } finally { expect((await grant(request, carol, "DELETE")).status()).toBe(204); }
});

test("ownership transfer is authorized and removes the former owner's access", async ({ page, request }) => {
  const alice = await token(request, "alice"), carol = await token(request, "carol"), dave = await token(request, "dave");
  await login(page, "carol");
  await page.locator("#object-id").fill("requests");
  await page.locator("#new-owner").fill("dave");
  try {
    await page.locator("#owner-form button").click();
    await expect(page.locator("#results")).toContainText(`${base}/requests/owner → 204`);
    await expect.poll(() => ids(page)).toEqual([]);
    expect((await request.get(`${base}/requests`, { headers: headers(carol) })).status()).toBe(403);
    const actual = await request.get(`${base}/requests`, { headers: headers(dave) });
    expect(actual.status()).toBe(200); expect((await actual.json()).owner_id).toBe("dave");
    await login(page, "dave");
    await page.locator("#object-id").fill("requests");
    await page.locator("#action").selectOption("can_change_ownership");
    await page.locator("#check-form button").click();
    await expect(page.locator("#decision")).toHaveText("ALLOW · can_change_ownership");
  } finally {
    expect((await request.put(`${base}/requests/owner`, { headers: headers(alice), data: { owner_id: "carol" } })).status()).toBe(204);
  }
});

test("role assignment and removal affect nested inherited access", async ({ page, request }) => {
  const alice = await token(request, "alice");
  const path = "/projects/acme/roles/analysts/assignees/dave";
  await login(page, "alice");
  await page.locator("#role-id").selectOption("analysts");
  await page.locator("#role-user").fill("dave");
  try {
    await page.locator('#role-form button[value="grant"]').click();
    await expect(page.locator("#results")).toContainText(`${path} → 204`);
    await login(page, "dave");
    await expect.poll(() => ids(page)).toEqual(["errors", "latency", "requests"]);
    expect((await request.delete(path, { headers: headers(alice) })).status()).toBe(204);
    await refresh(page, ["latency"]);
  } finally { expect((await request.delete(path, { headers: headers(alice) })).status()).toBe(204); }
});

test("cross-project substitutions and unprivileged moves fail without changing metadata", async ({ page, request }) => {
  const alice = await token(request, "alice"), carol = await token(request, "carol"), eve = await token(request, "eve");
  expect((await request.get("/projects/other/warehouses/external/tables/requests", { headers: headers(alice) })).status()).toBe(404);
  expect((await request.get(`${base}/requests`, { headers: headers(eve) })).status()).toBe(404);
  expect((await request.put(`${base}/requests/owner`, { headers: headers(carol), data: { owner_id: "eve" } })).status()).toBe(404);
  expect((await request.put(`${base}/requests/namespace`, { headers: headers(carol), data: { namespace_id: "restricted" } })).status()).toBe(403);
  const unchanged = await request.get(`${base}/requests`, { headers: headers(carol) });
  expect((await unchanged.json()).namespace_id).toBe("metrics");
  await login(page, "alice");
  await page.locator("#object-id").fill("requests");
  await page.locator("#new-namespace").selectOption("restricted");
  try {
    await page.locator("#move-form button").click();
    await expect(page.locator("#results")).toContainText(`${base}/requests/namespace → 204`);
    const moved = await request.get(`${base}/requests`, { headers: headers(alice) });
    expect((await moved.json()).namespace_id).toBe("restricted");
  } finally { expect((await request.put(`${base}/requests/namespace`, { headers: headers(alice), data: { namespace_id: "metrics" } })).status()).toBe(204); }
});
