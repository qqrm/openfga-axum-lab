const { test, expect } = require("@playwright/test");
const { base, login, token, headers, ids, refresh } = require("./helpers");

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


test("catalog provenance and protected responses retain no-store and browser security headers", async ({ page, request }) => {
  const document = await page.goto("/");
  expect(document.status()).toBe(200);
  expectProtectedHeaders(document);
  const provenance = await request.get("/model");
  expect(provenance.status()).toBe(200);
  expectProtectedHeaders(provenance);
  expect(await provenance.json()).toMatchObject({ project: "Lakekeeper", version: "4.15", license: "Apache-2.0", model_changes: false });
  const schema = await request.get("/model/schema");
  expect(schema.status()).toBe(200);
  expectProtectedHeaders(schema);
  const model = await schema.json();
  expect(model.schema_version).toBe("1.2");
  expect(model.type_definitions.map(definition => definition.type)).toEqual(expect.arrayContaining(["project", "warehouse", "namespace", "lakekeeper_table", "role"]));
  const dave = await token(request, "dave");
  for (const [path, bearer, status] of [[base, dave, 200], [`${base}/requests`, dave, 403], [base, null, 401]]) {
    const response = await request.get(path, { headers: bearer ? headers(bearer) : {} });
    expect(response.status()).toBe(status);
    expectProtectedHeaders(response);
  }
});

test("table creation in the UI, authorized edits and deletion keep real permissions", async ({ page, request }) => {
  const alice = await token(request, "alice"), dave = await token(request, "dave");
  await login(page, "alice");
  await page.getByText("Create a table", { exact: true }).click();
  await page.locator("#create-namespace").selectOption("restricted");
  await page.locator("#create-name").fill("Browser lifecycle");
  await page.locator("#create-description").fill("Real catalog lifecycle");
  const response = page.waitForResponse(response => new URL(response.url()).pathname === base && response.request().method() === "POST");
  await page.locator("#create-form button").click();
  const created = await response;
  expect(created.status()).toBe(201);
  const id = (await created.json()).id;
  let deleted = false;
  try {
    await expect.poll(() => ids(page)).toContain(id);
    expect((await request.patch(`${base}/${id}`, { headers: headers(dave), data: { name: "Forbidden", description: "" } })).status()).toBe(403);
    expect((await request.patch(`${base}/${id}`, { headers: headers(alice), data: { name: "Updated browser lifecycle", description: "Verified update" } })).status()).toBe(204);
    await page.locator("#refresh").click();
    await expect(page.locator("#tables")).toContainText("Updated browser lifecycle");
    expect((await request.delete(`${base}/${id}`, { headers: headers(alice) })).status()).toBe(204);
    deleted = true;
    await refresh(page, ["errors", "latency", "requests"]);
    expect((await request.get(`${base}/${id}`, { headers: headers(alice) })).status()).toBe(404);
  } finally {
    if (!deleted) expect((await request.delete(`${base}/${id}`, { headers: headers(alice) })).status()).toBe(204);
  }
});
