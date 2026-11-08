"use strict";
const $ = id => document.getElementById(id);
const state = { token: null, epoch: 0, scopes: [], cursor: null, nextCursor: null };
const encoded = value => encodeURIComponent(value);
const currentScope = () => state.scopes[Number($("scope").value)];
function basePath() {
  const scope = currentScope();
  if (!scope) throw new Error("Select a project and warehouse first.");
  return `/projects/${encoded(scope.project_id)}/warehouses/${encoded(scope.warehouse_id)}/tables`;
}
function objectPath() {
  const id = $("object-id").value.trim();
  if (!id) throw new Error("Enter a table ID first.");
  return `${basePath()}/${encoded(id)}`;
}
function result(message, ok) {
  const li = document.createElement("li");
  li.textContent = new Date().toLocaleTimeString() + "  " + message;
  li.className = ok ? "succeeded" : "failed";
  $("results").prepend(li);
  while ($("results").children.length > 30) $("results").lastChild.remove();
}
async function api(path, options = {}) {
  if (!state.token) throw new Error("Sign in before sending a protected request.");
  const epoch = state.epoch;
  const headers = { Authorization: "Bearer " + state.token, ...options.headers };
  if (options.body !== undefined) headers["Content-Type"] = "application/json";
  const response = await fetch(path, { ...options, headers, body: options.body === undefined ? undefined : JSON.stringify(options.body), cache: "no-store" });
  const body = response.status === 204 ? null : await response.json().catch(() => null);
  if (epoch !== state.epoch) throw new Error("Session or scope changed; discarded an earlier response.");
  const method = options.method || "GET";
  result(`${method} ${path} → ${response.status}`, response.ok);
  if (!response.ok) {
    const reason = response.status === 503 ? "Authorization unavailable or an operation is awaiting reconciliation. No successful decision was returned." : response.status === 403 ? "Access denied." : response.status === 404 ? "Object not found in this scope." : "Request rejected.";
    throw Object.assign(new Error(`${reason} HTTP ${response.status}`), { status: response.status });
  }
  return body;
}
function resetView() {
  state.epoch++; state.token = null; state.scopes = []; state.cursor = null; state.nextCursor = null;
  $("workspace").hidden = true; $("tables").replaceChildren(); $("scope").replaceChildren();
  $("object-details").hidden = true; $("object-details").textContent = ""; $("object-id").value = "";
  $("results").replaceChildren();
  for (const id of ["create-namespace", "new-namespace", "role-id"]) $(id).replaceChildren();
  for (const id of ["create-name", "create-description", "subject-id", "new-owner", "role-user"]) $(id).value = "";
  $("table-count").textContent = "0 on this page"; $("empty").hidden = true; $("next-page").hidden = true;
  $("decision").textContent = "No decision requested"; $("decision").className = "decision";
  $("session-state").textContent = "Signed out"; $("error").textContent = "";
}
async function guarded(work, form) {
  $("error").textContent = "";
  const buttons = form ? [...form.querySelectorAll("button")] : [];
  buttons.forEach(button => { button.disabled = true; });
  try { await work(); } catch (error) { $("error").textContent = error.message; }
  finally { buttons.forEach(button => { button.disabled = false; }); }
}
function options(id, values, label) {
  $(id).replaceChildren(...values.map(value => {
    const option = document.createElement("option"); option.value = value.id;
    option.textContent = label(value); return option;
  }));
}
async function loadPage() {
  const epoch = state.epoch;
  $("tables").replaceChildren(); $("next-page").hidden = true;
  const query = new URLSearchParams({ limit: "25" });
  if (state.cursor) query.set("cursor", state.cursor);
  const page = await api(`${basePath()}?${query}`);
  if (epoch !== state.epoch) return;
  if (!page || !Array.isArray(page.items)) throw new Error("Invalid catalog response; no rows displayed.");
  for (const item of page.items) {
    const row = document.createElement("tr");
    const name = document.createElement("td"); name.textContent = item.name;
    const id = document.createElement("small"); id.textContent = item.id; name.append(id);
    const namespace = document.createElement("td"); namespace.textContent = item.namespace_id;
    const owner = document.createElement("td"); owner.textContent = item.owner_id;
    const control = document.createElement("td"), button = document.createElement("button");
    button.className = "secondary"; button.textContent = "Inspect";
    button.addEventListener("click", () => guarded(async () => {
      $("object-id").value = item.id;
      const actual = await api(objectPath());
      $("object-details").textContent = JSON.stringify(actual, null, 2); $("object-details").hidden = false;
      $("decision").textContent = "Select an action and check access"; $("decision").className = "decision";
    }));
    control.append(button); row.append(name, namespace, owner, control); $("tables").append(row);
  }
  state.nextCursor = page.next_cursor || null;
  $("next-page").hidden = !state.nextCursor;
  $("empty").hidden = page.items.length !== 0;
  $("table-count").textContent = `${page.items.length} on this page`;
}
async function setScope() {
  state.epoch++;
  state.cursor = null; state.nextCursor = null;
  const scope = currentScope(); if (!scope) return;
  options("create-namespace", scope.namespaces, item => `${item.name} / ${item.id}`);
  options("new-namespace", scope.namespaces, item => `${item.name} / ${item.id}`);
  options("role-id", scope.roles, item => `${item.name} / ${item.id}`);
  $("object-id").value = ""; $("object-details").hidden = true;
  $("decision").textContent = "No decision requested"; $("decision").className = "decision";
  await loadPage();
}
$("login-form").addEventListener("submit", event => {
  event.preventDefault();
  guarded(async () => {
    resetView(); const epoch = state.epoch, username = $("who").value, password = $("password").value;
    $("password").value = "";
    const response = await fetch("/session", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ username, password }), cache: "no-store" });
    result(`POST /session → ${response.status}`, response.ok);
    if (!response.ok) throw new Error(`Sign-in failed. HTTP ${response.status}`);
    const body = await response.json();
    if (epoch !== state.epoch) return;
    if (typeof body.token !== "string" || !body.token) throw new Error("Invalid session response.");
    state.token = body.token;
    const catalog = await api("/catalog");
    if (!Array.isArray(catalog.scopes)) throw new Error("Invalid scope response.");
    state.scopes = catalog.scopes;
    $("scope").replaceChildren(...state.scopes.map((scope, index) => {
      const option = document.createElement("option"); option.value = String(index); option.textContent = `${scope.project_name} / ${scope.warehouse_name}`; return option;
    }));
    $("session-state").textContent = `Authenticated as ${username}`;
    $("workspace").hidden = state.scopes.length === 0; await setScope();
  }, event.currentTarget);
});
$("sign-out").addEventListener("click", () => guarded(async () => {
  try { if (state.token) await api("/session", { method: "DELETE" }); } finally { resetView(); }
}));
$("who").addEventListener("change", resetView);
$("scope").addEventListener("change", () => guarded(setScope));
$("refresh").addEventListener("click", () => guarded(async () => { state.cursor = null; await loadPage(); }));
$("next-page").addEventListener("click", () => guarded(async () => { state.cursor = state.nextCursor; await loadPage(); }));
$("check-form").addEventListener("submit", event => {
  event.preventDefault(); guarded(async () => {
    $("decision").className = "decision"; $("decision").textContent = "Checking…";
    try {
      const action = $("action").value;
      const decision = await api(`${objectPath()}/check`, { method: "POST", body: { action } });
      if (typeof decision.allowed !== "boolean") throw new Error("Invalid decision response.");
      $("decision").className = `decision ${decision.allowed ? "allow" : "deny"}`;
      $("decision").textContent = `${decision.allowed ? "ALLOW" : "DENY"} · ${action}`;
    } catch (error) {
      $("decision").className = "decision unavailable"; $("decision").textContent = "UNAVAILABLE / REJECTED · no allow"; throw error;
    }
  }, event.currentTarget);
});
$("create-form").addEventListener("submit", event => {
  event.preventDefault(); guarded(async () => {
    const created = await api(basePath(), { method: "POST", headers: { "Idempotency-Key": crypto.randomUUID() }, body: { namespace_id: $("create-namespace").value, name: $("create-name").value, description: $("create-description").value } });
    $("object-id").value = created.id; state.cursor = null; await loadPage();
  }, event.currentTarget);
});
$("grant-form").addEventListener("submit", event => {
  event.preventDefault(); const method = event.submitter.value === "revoke" ? "DELETE" : "PUT";
  guarded(async () => {
    await api(`${objectPath()}/grants/${encoded($("grant-relation").value)}`, { method, body: { subject: { type: $("subject-type").value, id: $("subject-id").value.trim() } } });
    state.cursor = null; await loadPage();
  }, event.currentTarget);
});
$("owner-form").addEventListener("submit", event => {
  event.preventDefault(); guarded(async () => {
    await api(`${objectPath()}/owner`, { method: "PUT", body: { owner_id: $("new-owner").value.trim() } }); await loadPage();
  }, event.currentTarget);
});
$("move-form").addEventListener("submit", event => {
  event.preventDefault(); guarded(async () => {
    await api(`${objectPath()}/namespace`, { method: "PUT", body: { namespace_id: $("new-namespace").value } }); await loadPage();
  }, event.currentTarget);
});
$("role-form").addEventListener("submit", event => {
  event.preventDefault(); const method = event.submitter.value === "revoke" ? "DELETE" : "PUT";
  guarded(async () => {
    const scope = currentScope();
    await api(`/projects/${encoded(scope.project_id)}/roles/${encoded($("role-id").value)}/assignees/${encoded($("role-user").value.trim())}`, { method });
    state.cursor = null; await loadPage();
  }, event.currentTarget);
});
$("clear-results").addEventListener("click", () => $("results").replaceChildren());
fetch("/model", { cache: "no-store" }).then(response => { if (!response.ok) throw new Error("Model provenance unavailable"); return response.json(); }).then(model => {
  $("model-name").textContent = `${model.project} model v${model.version} · ${model.commit.slice(0, 8)} · ${model.license}`;
  if (typeof model.source_url === "string" && model.source_url.startsWith("https://github.com/lakekeeper/lakekeeper/")) $("model-source").href = model.source_url;
}).catch(() => { $("model-name").textContent = "Model provenance unavailable"; });
