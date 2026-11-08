const { test, expect } = require("@playwright/test");
const { login, api, createTask, removeTask, refresh, selectTask, selectOrganization, mutate, setTaskRelation, setOrganizationPermission, expectIds, expectTaskEditable } = require("../support/lab");

test("role promotion and demotion change creation rights without making admin an owner", async ({ page, context, request }) => {
  const alice = await login(page, "alice");
  const member = await context.newPage();
  const carol = await login(member, "carol");
  let createdId;
  try {
    await selectOrganization(page, "acme");
    await expect(member.locator("#new-task")).toBeDisabled();
    await page.fill("#member-user", "carol");
    await page.selectOption("#member-role", "admin");
    await mutate(page, () => page.click("#set-member"), "PUT", "/organizations/acme/members/carol");
    await refresh(member);
    await expect(member.locator("#new-task")).toBeEnabled();
    // Promotion adds creation, not read-all. No direct grant was added here.
    await expectIds(member, ["access-review", "incident-response"]);
    await selectOrganization(member, "acme");
    await expect(member.locator("#organization-management")).toBeHidden();
    await api(request, "PUT", "/organizations/acme/members/dave", carol, { role: "admin" }, 403);
    await api(request, "PUT", "/organizations/acme/permissions/can_edit_task/dave", carol, undefined, 403);
    await api(request, "PUT", "/organizations/acme/owner", carol, { expected_owner: "alice", new_owner: "carol" }, 403);
    createdId = await createTask(request, carol, "Promoted administrator review");
    await refresh(member);
    await selectTask(member, createdId);
    await expect(member.locator("#delete-task")).toBeVisible();
    await page.fill("#member-user", "carol");
    await page.selectOption("#member-role", "member");
    await mutate(page, () => page.click("#set-member"), "PUT", "/organizations/acme/members/carol");
    await refresh(member);
    await expect(member.locator("#new-task")).toBeDisabled();
    // Demotion removes creation rights but not the independent assigner relation.
    await expect(member.locator("#task-context")).toContainText(createdId);
    await expect(member.locator("#delete-task")).toBeVisible();
    await api(request, "POST", "/tasks", carol, { organization_id: "acme", title: "Denied creation", description: "", status: "todo" }, 403);
  } finally {
    await removeTask(request, carol, createdId);
    await api(request, "PUT", "/organizations/acme/members/carol", alice, { role: "member" });
    await member.close();
  }
});

test("organization grants inherit into tasks and direct grants survive their revoke", async ({ page, context, request }) => {
  const alice = await login(page, "alice");
  const member = await context.newPage();
  const carol = await login(member, "carol");
  let id;
  try {
    id = await createTask(request, alice, "Inherited access review");
    await selectOrganization(page, "acme");
    await refresh(page);
    await selectTask(page, id);
    await api(request, "GET", "/tasks/" + id, carol, undefined, 403);
    await setOrganizationPermission(page, "can_view_task", "carol", true);
    await refresh(member);
    await selectTask(member, id);
    await expectTaskEditable(member, false);
    await expect(member.locator("#task-access")).toBeHidden();
    await setTaskRelation(page, id, "viewer", "carol", true);
    await setOrganizationPermission(page, "can_view_task", "carol", false);
    await refresh(member);
    await expect(member.locator('[data-task-id="' + id + '"]')).toHaveCount(1);
    await api(request, "GET", "/tasks/" + id, carol, undefined, 200);
    await setTaskRelation(page, id, "viewer", "carol", false);
    await refresh(member);
    await expect(member.locator('[data-task-id="' + id + '"]')).toHaveCount(0);
    await api(request, "GET", "/tasks/" + id, carol, undefined, 403);
  } finally {
    await api(request, "DELETE", "/organizations/acme/permissions/can_view_task/carol", alice);
    await removeTask(request, alice, id);
    await member.close();
  }
});

test("owner transfer removes inherited privileges and the new owner can transfer back", async ({ page, context, request }) => {
  const alice = await login(page, "alice");
  const successor = await context.newPage();
  const carol = await login(successor, "carol");
  try {
    await api(request, "PUT", "/organizations/acme/owner", alice, { expected_owner: "bob", new_owner: "carol" }, 409);
    await selectOrganization(page, "acme");
    await selectTask(page, "access-review");
    await expect(page.locator("#organization-management")).toBeVisible();
    await page.selectOption("#new-owner", "carol");
    page.once("dialog", (dialog) => dialog.accept());
    await mutate(page, () => page.click("#transfer-owner-submit"), "PUT", "/organizations/acme/owner");
    await expect(page.locator("#org-owner")).toHaveText("Владелец: carol");
    await expect(page.locator("#organization-management")).toBeHidden();
    await expectIds(page, ["vendor-review"]);
    await expect(page.locator("#task-detail")).toBeHidden();
    await api(request, "GET", "/tasks/access-review", alice, undefined, 403);
    // Former ownership is not enough to transfer or grant privileges again.
    await api(request, "PUT", "/organizations/acme/owner", alice, { expected_owner: "carol", new_owner: "alice" }, 403);
    await api(request, "PUT", "/organizations/acme/members/dave", alice, { role: "admin" }, 403);
    // Alice remains the assigner of vendor-review, independently of ownership.
    await api(request, "GET", "/tasks/vendor-review", alice, undefined, 200);
    await refresh(successor);
    await expectIds(successor, ["access-review", "vendor-review", "incident-response"]);
    await selectOrganization(successor, "acme");
    await expect(successor.locator("#organization-management")).toBeVisible();
    await successor.selectOption("#new-owner", "alice");
    successor.once("dialog", (dialog) => dialog.accept());
    await mutate(successor, () => successor.click("#transfer-owner-submit"), "PUT", "/organizations/acme/owner");
    await expect(successor.locator("#org-owner")).toHaveText("Владелец: alice");
    await expect(successor.locator("#organization-management")).toBeHidden();
    await refresh(page);
    await expect(page.locator("#organization-management")).toBeVisible();
    await expectIds(page, ["access-review", "vendor-review", "incident-response"]);
  } finally {
    // Restore only this test's transfer if an assertion failed halfway through.
    const response = await api(request, "GET", "/organizations", carol, undefined, 200);
    const organization = (await response.json()).find((org) => org.id === "acme");
    if (organization?.owner_id === "carol") {
      await api(request, "PUT", "/organizations/acme/owner", carol, { expected_owner: "carol", new_owner: "alice" });
    }
    await successor.close();
  }
});
