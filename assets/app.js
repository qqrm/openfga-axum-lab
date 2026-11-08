"use strict";

// The browser never evaluates the authorization model. It displays only data
// and capability flags returned by Axum; every write is checked again there.
const byId = (id) => document.getElementById(id);
const statusLabels = { todo: "К выполнению", in_progress: "В работе", done: "Готово" };
const state = {
  token: null, username: null, epoch: 0, view: 0, busy: false,
  organizations: [], tasks: [], members: [], organization: "*", selectedTask: null,
};
let createIntent = null;

class Cancelled extends Error {}
class ApiError extends Error {
  constructor(status) {
    const messages = {
      400: "Проверьте поля запроса.",
      401: "Сеанс недействителен. Войдите снова.",
      403: "У пользователя нет разрешения на эту операцию.",
      404: "Объект не найден или больше недоступен.",
      409: "Состояние изменилось или операция конфликтует с текущими данными. Обновите страницу данных.",
      410: "Задача уже удалена.",
      429: "Слишком много попыток. Попробуйте позже.",
      503: "Сервис не может подтвердить доступ или завершить изменение. Данные скрыты. Дождитесь восстановления; если ошибка сохраняется, обратитесь к оператору стенда.",
    };
    super((messages[status] || "Сервис не смог выполнить запрос.") + " (HTTP " + status + ")");
    this.status = status;
  }
}

function node(tag, text, className) {
  const element = document.createElement(tag);
  if (text !== undefined) element.textContent = text;
  if (className) element.className = className;
  return element;
}
function option(value, text) {
  const element = node("option", text);
  element.value = value;
  return element;
}
function currentOrganization() {
  return state.organizations.find((organization) => organization.id === state.organization);
}
function clearDetail() {
  state.selectedTask = null;
  byId("task-detail").hidden = true;
  byId("task-access").hidden = true;
  byId("task-context").textContent = "";
  byId("task-heading").textContent = "";
  byId("task-recovery").hidden = true;
  byId("task-permissions").textContent = "";
  byId("direct-relations").replaceChildren();
  byId("edit-task").reset();
  byId("edit-fields").disabled = true;
  byId("delete-task").hidden = true;
  byId("task-relation").reset();
}
function clearData() {
  state.view += 1;
  state.organizations = [];
  state.tasks = [];
  state.members = [];
  state.organization = "*";
  byId("organization").replaceChildren(option("*", "Все доступные задачи"));
  byId("tasks").replaceChildren();
  byId("task-count").textContent = "";
  byId("members").replaceChildren();
  byId("org-heading").textContent = "";
  byId("org-owner").textContent = "";
  byId("new-owner").replaceChildren();
  byId("create-organization").replaceChildren();
  byId("create-panel").hidden = true;
  byId("organization-panel").hidden = true;
  byId("organization-recovery").hidden = true;
  byId("organization-management").hidden = true;
  byId("new-task").disabled = true;
  byId("empty-tasks").hidden = true;
  clearDetail();
}
function resetSession() {
  state.epoch += 1;
  state.token = null;
  state.username = null;
  state.busy = false;
  createIntent = null;
  clearData();
  byId("workspace").hidden = true;
  byId("workspace").inert = false;
  byId("session-status").textContent = "Вход не выполнен";
  byId("sign-out").disabled = true;
  byId("sign-in").disabled = false;
  byId("password").value = "";
  byId("create-task").reset();
  byId("member-form").reset();
  byId("organization-permission").reset();
}
function fail(error) {
  if (error instanceof Cancelled) return;
  if (error.status === 401) resetSession();
  else if ([403, 404, 410].includes(error.status) || error.status >= 500 || !(error instanceof ApiError)) clearData();
  byId("error").textContent = error instanceof ApiError ? error.message : "Не удалось связаться с сервисом. Обновите данные после восстановления соединения.";
}
async function call(path, { method = "GET", data, idempotencyKey } = {}) {
  const epoch = state.epoch;
  const token = state.token;
  if (!token) throw new ApiError(401);
  const headers = { Authorization: "Bearer " + token };
  if (data !== undefined) headers["Content-Type"] = "application/json";
  if (idempotencyKey) headers["Idempotency-Key"] = idempotencyKey;
  const response = await fetch(path, { method, headers, body: data === undefined ? undefined : JSON.stringify(data), cache: "no-store" });
  if (epoch !== state.epoch || token !== state.token) throw new Cancelled();
  if (!response.ok) throw new ApiError(response.status);
  const result = response.status === 204 ? null : await response.json();
  if (epoch !== state.epoch || token !== state.token) throw new Cancelled();
  return result;
}
async function perform(action, message) {
  if (state.busy || !state.token) return;
  const epoch = state.epoch;
  state.busy = true;
  byId("workspace").inert = true;
  byId("error").textContent = "";
  byId("notice").textContent = "";
  try {
    await action();
    if (epoch === state.epoch && message) byId("notice").textContent = message;
  } catch (error) {
    if (epoch === state.epoch) fail(error);
  } finally {
    if (epoch === state.epoch) {
      state.busy = false;
      byId("workspace").inert = false;
    }
  }
}
function renderTasks() {
  const tasks = state.tasks.filter((task) => state.organization === "*" || task.organization_id === state.organization);
  const list = byId("tasks");
  list.replaceChildren();
  byId("empty-tasks").hidden = tasks.length !== 0;
  byId("task-count").textContent = "Доступно задач: " + tasks.length;
  for (const task of tasks) {
    const item = node("li");
    const button = node("button", undefined, "task-button");
    button.type = "button";
    button.dataset.taskId = task.id;
    button.setAttribute("aria-current", String(state.selectedTask?.id === task.id));
    const title = node("span", task.title, "task-title");
    const meta = node("span", undefined, "task-meta");
    meta.append(node("span", task.organization_id + " / " + task.id, "task-id"));
    const knownStatus = Object.hasOwn(statusLabels, task.status) ? task.status : "todo";
    meta.append(node("span", statusLabels[task.status] || task.status, "status-chip status-" + knownStatus));
    button.append(title, meta);
    button.addEventListener("click", () => perform(() => loadTask(task.id)));
    item.append(button);
    list.append(item);
  }
}
async function loadTask(id) {
  clearDetail();
  const view = state.view;
  const task = await call("/tasks/" + encodeURIComponent(id));
  if (view !== state.view) throw new Cancelled();
  state.selectedTask = task;
  byId("task-context").textContent = task.organization_id + " / " + task.id + " · assigner: " + task.assigner_id;
  byId("task-heading").textContent = task.title;
  byId("task-recovery").hidden = task.recovery_required !== true;
  const capabilities = ["Чтение"];
  if (task.can_edit === true) capabilities.push("редактирование");
  if (task.can_delete === true) capabilities.push("удаление и выдача прав");
  byId("task-permissions").textContent = "Подтверждённые права: " + capabilities.join(" · ");
  byId("edit-title").value = task.title;
  byId("edit-description").value = task.description;
  byId("edit-status").value = task.status;
  byId("edit-fields").disabled = task.can_edit !== true;
  byId("delete-task").hidden = task.can_delete !== true;
  byId("task-access").hidden = task.can_delete !== true;
  if (task.can_delete === true && Array.isArray(task.relations)) {
    const list = byId("direct-relations");
    for (const relation of task.relations) list.append(node("li", relation.relation + ": " + relation.user_id));
    list.hidden = task.relations.length === 0;
  }
  byId("task-detail").hidden = false;
  renderTasks();
}
async function loadMembers() {
  byId("organization-panel").hidden = true;
  byId("organization-recovery").hidden = true;
  byId("organization-management").hidden = true;
  byId("members").replaceChildren();
  state.members = [];
  const organization = currentOrganization();
  if (!organization) return;
  const members = await call("/organizations/" + encodeURIComponent(organization.id) + "/members");
  if (state.organization !== organization.id) throw new Cancelled();
  state.members = members;
  byId("org-heading").textContent = organization.name;
  byId("org-owner").textContent = "Владелец: " + organization.owner_id;
  byId("organization-recovery").hidden = organization.recovery_required !== true;
  byId("new-owner").replaceChildren();
  for (const member of members) {
    const row = node("tr");
    row.dataset.userId = member.user_id;
    const user = node("td", member.display_name);
    user.append(node("span", member.user_id, "member-id"));
    row.append(user, node("td", member.role), node("td", member.is_owner ? "owner" : "—"));
    byId("members").append(row);
    if (!member.is_owner) byId("new-owner").append(option(member.user_id, member.display_name + " (" + member.user_id + ")"));
  }
  byId("transfer-owner-submit").disabled = byId("new-owner").options.length === 0;
  byId("organization-management").hidden = organization.can_manage !== true;
  byId("organization-panel").hidden = false;
}
async function refreshWorkspace(selectedId = state.selectedTask?.id) {
  const organizationId = state.organization;
  const view = ++state.view;
  clearDetail();
  // Never leave old allow results visible while a refresh checks current rights.
  byId("tasks").replaceChildren();
  byId("organization-panel").hidden = true;
  const [organizations, tasks] = await Promise.all([call("/organizations"), call("/tasks")]);
  if (view !== state.view) throw new Cancelled();
  state.organizations = organizations;
  state.tasks = tasks;
  state.organization = organizations.some((org) => org.id === organizationId) ? organizationId : "*";
  const select = byId("organization");
  select.replaceChildren(option("*", "Все доступные задачи"));
  for (const org of organizations) select.append(option(org.id, org.name));
  select.value = state.organization;
  const creatable = organizations.filter((org) => org.can_create_task === true);
  const createSelect = byId("create-organization");
  createSelect.replaceChildren(...creatable.map((org) => option(org.id, org.name)));
  if (creatable.some((org) => org.id === state.organization)) createSelect.value = state.organization;
  byId("new-task").disabled = creatable.length === 0;
  if (!creatable.length) byId("create-panel").hidden = true;
  renderTasks();
  await loadMembers();
  if (selectedId && tasks.some((task) => task.id === selectedId)) await loadTask(selectedId);
}
async function closeRemoteSession(token) {
  if (!token) return;
  const response = await fetch("/session", { method: "DELETE", headers: { Authorization: "Bearer " + token }, cache: "no-store" });
  if (!response.ok && response.status !== 401) throw new ApiError(response.status);
}
byId("session-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const username = byId("who").value;
  const password = byId("password").value;
  const previousToken = state.token;
  resetSession();
  const epoch = state.epoch;
  byId("sign-in").disabled = true;
  byId("error").textContent = "";
  byId("notice").textContent = "";
  try {
    await closeRemoteSession(previousToken);
    const response = await fetch("/session", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ username, password }), cache: "no-store" });
    if (!response.ok) throw new ApiError(response.status);
    const session = await response.json();
    if (epoch !== state.epoch) {
      await closeRemoteSession(session.token);
      return;
    }
    state.token = session.token;
    state.username = username;
    byId("session-status").textContent = "Сеанс: " + username;
    byId("workspace").hidden = false;
    byId("sign-out").disabled = false;
    await perform(() => refreshWorkspace());
  } catch (error) {
    if (epoch === state.epoch) {
      fail(error);
      if (error.status === 401) byId("error").textContent = "Не удалось войти: неверный пользователь или пароль (HTTP 401).";
    }
  } finally {
    if (epoch === state.epoch) byId("sign-in").disabled = false;
  }
});
byId("sign-out").addEventListener("click", async () => {
  const token = state.token;
  resetSession();
  const epoch = state.epoch;
  byId("error").textContent = "";
  byId("notice").textContent = "";
  try { await closeRemoteSession(token); }
  catch (_) { if (epoch === state.epoch) byId("error").textContent = "Данные убраны из интерфейса, но сервер не подтвердил отзыв сеанса. Повторите вход и выход после восстановления сервиса."; }
});
byId("who").addEventListener("change", async () => {
  const token = state.token;
  resetSession();
  const epoch = state.epoch;
  byId("error").textContent = "";
  byId("notice").textContent = "";
  try { await closeRemoteSession(token); }
  catch (_) { if (epoch === state.epoch) byId("error").textContent = "Сервер не подтвердил отзыв предыдущего сеанса. Его данные больше не отображаются."; }
});
byId("refresh").addEventListener("click", () => perform(() => refreshWorkspace()));
byId("organization").addEventListener("change", () => perform(async () => {
  state.organization = byId("organization").value;
  clearDetail();
  renderTasks();
  await loadMembers();
}));
byId("new-task").addEventListener("click", () => {
  byId("create-panel").hidden = false;
  if (currentOrganization()?.can_create_task) byId("create-organization").value = state.organization;
  byId("create-title").focus();
});
byId("cancel-create").addEventListener("click", () => { byId("create-panel").hidden = true; });
byId("create-task").addEventListener("submit", (event) => {
  event.preventDefault();
  perform(async () => {
    const input = { organization_id: byId("create-organization").value, title: byId("create-title").value, description: byId("create-description").value, status: byId("create-status").value };
    const body = JSON.stringify(input);
    // A retry after a lost response reuses the same key for unchanged input.
    if (!createIntent || createIntent.body !== body) createIntent = { body, key: crypto.randomUUID() };
    const task = await call("/tasks", { method: "POST", data: input, idempotencyKey: createIntent.key });
    createIntent = null;
    byId("create-task").reset();
    byId("create-panel").hidden = true;
    state.organization = input.organization_id;
    await refreshWorkspace(task.id);
  }, "Задача создана. Права и список обновлены.");
});
byId("edit-task").addEventListener("submit", (event) => {
  event.preventDefault();
  perform(async () => {
    const task = state.selectedTask;
    if (!task) throw new Cancelled();
    await call("/tasks/" + encodeURIComponent(task.id), { method: "PATCH", data: { title: byId("edit-title").value, description: byId("edit-description").value, status: byId("edit-status").value } });
    await refreshWorkspace(task.id);
  }, "Задача сохранена.");
});
byId("delete-task").addEventListener("click", () => {
  const task = state.selectedTask;
  if (!task || !confirm("Удалить задачу «" + task.title + "» и её отношения доступа?")) return;
  perform(async () => {
    await call("/tasks/" + encodeURIComponent(task.id), { method: "DELETE" });
    await refreshWorkspace(null);
  }, "Задача удалена.");
});
byId("task-relation").addEventListener("submit", (event) => {
  event.preventDefault();
  const add = event.submitter?.value !== "revoke";
  perform(async () => {
    const task = state.selectedTask;
    if (!task) throw new Cancelled();
    const path = "/tasks/" + encodeURIComponent(task.id) + "/relations/" + encodeURIComponent(byId("relation").value) + "/" + encodeURIComponent(byId("relation-user").value);
    await call(path, { method: add ? "PUT" : "DELETE" });
    await refreshWorkspace(task.id);
  }, add ? "Отношение выдано. Решения о доступе обновлены." : "Прямое отношение отозвано. Независимые и наследуемые права могут сохраняться.");
});
byId("member-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const remove = event.submitter?.value === "remove";
  perform(async () => {
    const org = currentOrganization();
    if (!org) throw new Cancelled();
    const path = "/organizations/" + encodeURIComponent(org.id) + "/members/" + encodeURIComponent(byId("member-user").value);
    await call(path, remove ? { method: "DELETE" } : { method: "PUT", data: { role: byId("member-role").value } });
    await refreshWorkspace();
  }, remove ? "Членство отозвано. Проверьте отдельно независимые права на задачи." : "Роль участника обновлена.");
});
byId("organization-permission").addEventListener("submit", (event) => {
  event.preventDefault();
  const add = event.submitter?.value !== "revoke";
  perform(async () => {
    const org = currentOrganization();
    if (!org) throw new Cancelled();
    const path = "/organizations/" + encodeURIComponent(org.id) + "/permissions/" + encodeURIComponent(byId("organization-relation").value) + "/" + encodeURIComponent(byId("permission-user").value);
    await call(path, { method: add ? "PUT" : "DELETE" });
    await refreshWorkspace();
  }, add ? "Право организации выдано; оно наследуется её задачами." : "Прямое право организации отозвано. Другие основания доступа проверяются отдельно.");
});
byId("transfer-owner").addEventListener("submit", (event) => {
  event.preventDefault();
  const org = currentOrganization();
  const newOwner = byId("new-owner").value;
  if (!org || !newOwner || !confirm("Передать владение организацией «" + org.name + "» пользователю " + newOwner + "? Вы потеряете полномочия owner.")) return;
  perform(async () => {
    await call("/organizations/" + encodeURIComponent(org.id) + "/owner", { method: "PUT", data: { expected_owner: org.owner_id, new_owner: newOwner } });
    await refreshWorkspace();
  }, "Владение передано. Отображаются права после передачи.");
});
