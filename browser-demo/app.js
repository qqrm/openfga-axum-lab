"use strict";

// This page deliberately does not reimplement OpenFGA in JavaScript. Only the
// production lifecycle functions are executable here; the role table is prose.
async function start() {
  const response = await fetch("domain.wasm");
  if (!response.ok) throw new Error("Не удалось загрузить Rust-модуль жизненного цикла.");
  const { instance } = await WebAssembly.instantiate(await response.arrayBuffer());
  const { is_visible, next_state } = instance.exports;
  if (next_state(1, 1) !== 2 || is_visible(2) !== 0 || next_state(3, 0) !== -1) {
    throw new Error("Проверка Rust-модуля не прошла. Интерактивная часть отключена.");
  }
  let state = 0;
  const advance = document.getElementById("advance");
  const restart = document.getElementById("restart");
  function draw() {
    for (const item of document.querySelectorAll("[data-state]")) {
      const current = Number(item.dataset.state) === state;
      item.classList.toggle("current", current);
      if (current) item.setAttribute("aria-current", "step");
      else item.removeAttribute("aria-current");
    }
    const visible = is_visible(state) === 1;
    document.getElementById("lifecycle-result").textContent = visible
      ? "active: объект можно передать на проверку прав OpenFGA; это ещё не разрешение доступа."
      : ["pending: объект скрыт до подтверждения связей.", "", "deleting: объект уже скрыт, даже если удаление связей ещё не завершено.", "deleted: объект скрыт навсегда. Переход обратно в active запрещён."][state];
    advance.textContent = ["Подтвердить создание", "Начать удаление", "Подтвердить удаление связей", "Удаление завершено"][state];
    advance.disabled = state === 3;
    restart.disabled = state === 0;
  }
  advance.addEventListener("click", () => {
    const next = next_state(state, state);
    if (next < 0) {
      document.getElementById("error").textContent = "Rust отклонил переход состояния.";
      advance.disabled = true;
      return;
    }
    state = next;
    draw();
  });
  restart.addEventListener("click", () => {
    // A separate fixture, not resurrection of the deleted object.
    state = 0;
    draw();
  });
  document.getElementById("wasm-status").textContent = "Rust / WASM загружен";
  draw();
}
start().catch((error) => {
  document.getElementById("wasm-status").textContent = "WASM недоступен";
  document.getElementById("error").textContent = error.message;
});
