const { test, expect } = require("@playwright/test");

const PASSWORD = process.env.DEMO_PASSWORD;
if (!PASSWORD) {
  throw new Error("Set DEMO_PASSWORD to the password used by the running lab before running Playwright.");
}

async function login(page, username) {
  await page.selectOption("#who", username);
  await page.fill("#password", PASSWORD);
  await page.click("#sign-in");
}

async function petNames(page) {
  await page.waitForTimeout(300);
  return page.locator("#pets .card h2").allTextContents();
}

test("alice sees all seeded pets", async ({ page }) => {
  await page.goto("/");
  await login(page, "alice");
  await expect.poll(async () => (await petNames(page)).sort()).toEqual(["Mika", "Pixel", "Rex"]);
});

test("carol sees only Mika", async ({ page }) => {
  await page.goto("/");
  await login(page, "carol");
  await expect.poll(async () => await petNames(page)).toEqual(["Mika"]);
});

test("dave initially sees only Pixel", async ({ page }) => {
  await page.goto("/");
  await login(page, "dave");
  await expect.poll(async () => await petNames(page)).toEqual(["Pixel"]);
});

test("invalid password shows an error", async ({ page }) => {
  await page.goto("/");
  await page.selectOption("#who", "alice");
  await page.fill("#password", "wrong-password");
  await page.click("#sign-in");
  await expect(page.locator("#error")).toContainText("Не удалось войти");
});

test("granting and revoking caregiver access changes visible pets", async ({ page, request }) => {
  const carol = await request.post("/session", {
    data: { username: "carol", password: PASSWORD },
  });
  const carolToken = (await carol.json()).token;
  const dave = await request.post("/session", {
    data: { username: "dave", password: PASSWORD },
  });
  const daveToken = (await dave.json()).token;

  // Ensure clean state.
  await request.delete("/pets/fox/relations/caregiver/dave", {
    headers: { Authorization: "Bearer " + carolToken },
  });

  await page.goto("/");
  await login(page, "dave");
  await expect.poll(async () => await petNames(page)).toEqual(["Pixel"]);

  await request.put("/pets/fox/relations/caregiver/dave", {
    headers: { Authorization: "Bearer " + carolToken },
  });

  await page.click("#sign-out");
  await login(page, "dave");
  await expect.poll(async () => await petNames(page)).toEqual(["Pixel", "Mika"]);

  await request.delete("/pets/fox/relations/caregiver/dave", {
    headers: { Authorization: "Bearer " + carolToken },
  });

  await page.click("#sign-out");
  await login(page, "dave");
  await expect.poll(async () => await petNames(page)).toEqual(["Pixel"]);
});
