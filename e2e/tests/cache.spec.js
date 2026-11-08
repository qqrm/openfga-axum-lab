const { test, expect } = require("@playwright/test");

const PASSWORD = process.env.DEMO_PASSWORD;
if (!PASSWORD) {
  throw new Error("Set DEMO_PASSWORD to the password used by the running lab before running Playwright.");
}

async function token(request, username) {
  const response = await request.post("/session", {
    data: { username, password: PASSWORD },
  });
  return (await response.json()).token;
}

async function cleanRelation(request, carolToken) {
  await request.delete("/pets/fox/relations/caregiver/dave", {
    headers: { Authorization: "Bearer " + carolToken },
  });
}

async function signIn(page, username) {
  await page.selectOption("#who", username);
  await page.fill("#password", PASSWORD);
  await page.click("#sign-in");
}

async function petNames(page) {
  await page.waitForTimeout(200);
  return page.locator("#pets .card h2").allTextContents();
}

test("revoked access disappears from UI within cache TTL", async ({ page, request }) => {
  const carolToken = await token(request, "carol");
  await cleanRelation(request, carolToken);
  await request.put("/pets/fox/relations/caregiver/dave", {
    headers: { Authorization: "Bearer " + carolToken },
  });

  await page.goto("/");
  await signIn(page, "dave");
  await expect.poll(async () => await petNames(page)).toContain("Mika");

  const started = Date.now();
  await request.delete("/pets/fox/relations/caregiver/dave", {
    headers: { Authorization: "Bearer " + carolToken },
  });

  await page.click("#sign-out");
  await signIn(page, "dave");
  await expect.poll(async () => await petNames(page)).not.toContain("Mika");
  const elapsed = Date.now() - started;
  console.log("revocation observed in", elapsed, "ms");
  expect(elapsed).toBeLessThan(3000);
});

test("cached allow does not survive revoke after reload", async ({ page, request }) => {
  const carolToken = await token(request, "carol");
  await cleanRelation(request, carolToken);
  await request.put("/pets/fox/relations/caregiver/dave", {
    headers: { Authorization: "Bearer " + carolToken },
  });

  await page.goto("/");
  await signIn(page, "dave");
  await expect.poll(async () => await petNames(page)).toContain("Mika");

  // Warm the cache with a second load.
  await page.click("#sign-out");
  await signIn(page, "dave");
  await expect.poll(async () => await petNames(page)).toContain("Mika");

  await request.delete("/pets/fox/relations/caregiver/dave", {
    headers: { Authorization: "Bearer " + carolToken },
  });

  await page.click("#sign-out");
  await signIn(page, "dave");
  await expect.poll(async () => await petNames(page)).not.toContain("Mika");
});

test("reader cannot see a pet after permission is removed", async ({ page, request }) => {
  const carolToken = await token(request, "carol");
  await cleanRelation(request, carolToken);

  await page.goto("/");
  await signIn(page, "dave");
  await expect.poll(async () => await petNames(page)).toEqual(["Pixel"]);
});
