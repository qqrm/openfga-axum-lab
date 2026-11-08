# Лапки — Rust/Axum и объектная авторизация OpenFGA

Инженерная лаборатория: роли и доступ к отдельным объектам, настоящий
PostgreSQL/OpenFGA, отзыв прав между репликами и восстановление после отказов.
Все данные вымышлены. Это не публичная production IAM-система.

**[Открыть личную настоящую лабораторию в Codespaces](https://codespaces.new/qqrm/openfga-axum-lab?quickstart=1)** ·
**[Запустить временный стенд через Actions](https://github.com/qqrm/openfga-axum-lab/actions/workflows/live-lab.yml)** ·
[Посмотреть статическое демо](https://qqrm.github.io/openfga-axum-lab/)

## Посмотреть и потестировать в браузере

**Codespaces — для посетителя репозитория.** Нужен GitHub-аккаунт с доступной
квотой Codespaces; использование сверх квоты может оплачиваться. После создания
окружения `postStartCommand` собирает и запускает реальные Axum, PostgreSQL и
OpenFGA, проверяет готовность и первичный сценарий авторизации. Откройте порт
**3000** в панели **Ports**. Он приватный, а PostgreSQL/OpenFGA не пробрасываются.
В своём терминале выполните `cat .lab/codespaces/secrets/demo-password` и войдите
как `alice`, `bob`, `carol` или `dave`. Не публикуйте пароль. Данные сохраняются
при остановке/возобновлении; удаление Codespace удаляет его среду.

**Actions — для ручной проверки владельцем.** Один раз задайте repository secrets
`LIVE_LAB_ALLOWED_EMAIL` (один точный email) и `LIVE_LAB_PASSWORD` (16–128 символов).
Откройте **Live laboratory → Run workflow → main**, выберите 15, 30 или 60 минут.
После сборки и проверки в логе шага **Start and open the protected lab** появится
HTTPS-ссылка и время окончания. Войдите через email-код, затем используйте
demo-пользователя и свой `LIVE_LAB_PASSWORD`. Доступ работает, пока выполняется
**Keep the session available**. **Cancel workflow** закрывает сессию раньше.
Ссылка также сохраняется в summary и артефакте `live-lab-link-*`; старые ссылки
после завершения не работают. Summary может появиться только после окончания
job — во время сессии используйте ссылку из лога завершённого стартового шага.

До слияния PR #12 эти конфигурации находятся в `feature/goal-10-delivery`:
для Codespaces выберите эту ветку; ручной Actions workflow станет доступен
с `main` после слияния. Наличие конфигурации не означает созданного личного
Codespace или уже работающей сессии. Подробности и границы проверок:
[живая лаборатория](docs/live-lab.md).

**Pages — только имитация интерфейса и данных с WASM-ядром.** Настоящие
PostgreSQL и OpenFGA там не запускаются. Постоянные удалённые staging/stable
пока не настроены; для просмотра лабы они не обязательны.

## Локальный запуск

Нужны Git, Docker Engine, Docker Compose v2 и OpenSSL. Rust на хосте не нужен.
Команды для нового локального стенда в Linux/WSL2:

```sh
git clone https://github.com/qqrm/openfga-axum-lab.git
cd openfga-axum-lab
export POSTGRES_PASSWORD="$(openssl rand -hex 24)"
export DEMO_PASSWORD="$(openssl rand -hex 24)"
docker compose up -d --build app
```

Пароли создаются один раз. Экспортированные переменные нужны всем последующим
командам Compose в этой оболочке, включая диагностику и остановку. Сохраните
значения у себя и восстановите их при открытии новой оболочки. Не генерируйте
новые значения для существующих томов PostgreSQL.

Откройте `http://localhost:3000`. Локальный исходный стенд использует пароль
из `DEMO_PASSWORD` (посмотреть в своём терминале: `printenv DEMO_PASSWORD`):
`alice` — администратор, `bob` — сотрудник,
`carol` — владелец лисы Mika, `dave` изначально видит кота Pixel, но не лису.
Это не пароль личного Codespace или защищённой Actions-сессии.

Compose создаёт две базы в PostgreSQL; Rust `lab-tool bootstrap` инициализирует
модель/отношения OpenFGA. Повторный запуск сохраняет store/model.
`APP_PORT=3001 docker compose up -d app` меняет порт. `DEMO_PASSWORD` задаёт
только первоначальный пароль: существующие аккаунты переменная не изменяет.

## Проверить основную задачу

На свежем изолированном демонстрационном стенде:

```sh
docker compose exec -e SMOKE_ALLOW_MUTATIONS=true app lab-tool acceptance
```

Ожидается JSON с `accepted: true` и последовательностью
**403 → 204 → 200 → 204 → 403**: `dave` не читает лису, `carol` выдаёт доступ,
чтение проходит, доступ отзывается, чтение снова запрещено. Команда изменяет
тестовые права и возвращает их в исходное состояние; не запускайте её параллельно
с ручным редактированием этих же прав. Предварительно выданный доступ — ошибка,
а не повод молча его стереть. В Codespaces: `bash scripts/codespace-lab.sh verify`.

Диагностика локального Compose:

```sh
docker compose ps
docker compose logs --tail=100 app bootstrap openfga postgres
curl --fail http://localhost:3000/healthz
curl --fail http://localhost:3000/readyz
curl --fail http://localhost:3000/version
```

`/healthz` проверяет процесс; `/readyz` — PostgreSQL, OpenFGA и конкретные
store/model; `/version` — пакет и встроенный source SHA. Не используйте
liveness как доказательство готовности авторизации.
`docker compose down` сохраняет данные. **Только для полного сброса своего
локального стенда:** `docker compose down -v` удаляет все его тома.

## Поставка и тесты

Запуск готового версионированного образа без сборки Rust, подготовка независимых
окружений, release/deploy workflows и ограничения отката:
[поставка](docs/delivery.md). Образ выбирается по digest, не по `latest`.
Опубликованный GHCR-образ и релиз не заявляются до их фактической публикации.

Для разработки нужны Rust, Docker/Compose и nextest:

```sh
cargo install cargo-nextest --locked
cargo nextest run --locked --profile ci
cargo nextest run --locked --release --profile ci --run-ignored all \
  -E 'not test(paired_aa_calibration) and not binary(delivery)' --success-output immediate
```

В Codespace установлен Rust 1.99; для фиксированной версии используйте
`cargo +1.99.0`. Nextest устанавливается приведённой командой, не скрытым
скачиванием при каждом запуске лабы.

| Workflow | Проверки и результаты |
| --- | --- |
| [CI](https://github.com/qqrm/openfga-axum-lab/actions/workflows/ci.yml) | fmt, Clippy, nextest, реальные интеграции, Kani, зависимости; JUnit в артефактах |
| [Delivery verification](https://github.com/qqrm/openfga-axum-lab/actions/workflows/delivery.yml) | Контейнер на двух одноразовых стендах, изоляция, dump/restore; `delivery-evidence.json` |
| [Browser lab verification](https://github.com/qqrm/openfga-axum-lab/actions/workflows/browser-lab-check.yml) | Настоящий devcontainer, Docker-in-Docker и повторный запуск; это не проверка входа в GitHub Codespaces |
| [Live laboratory](https://github.com/qqrm/openfga-axum-lab/actions/workflows/live-lab.yml) | Только ручной ограниченный сеанс, закрытый email-кодом |
| [Workflow Security](https://github.com/qqrm/openfga-axum-lab/actions/workflows/workflow-security.yml) | actionlint, zizmor и тесты политики |
| [Secret scan](https://github.com/qqrm/openfga-axum-lab/actions/workflows/secrets.yml) / [CodeQL](https://github.com/qqrm/openfga-axum-lab/actions/workflows/codeql.yml) | Исходники и секреты |
| [Manual performance calibration](https://github.com/qqrm/openfga-axum-lab/actions/workflows/performance.yml) | Отдельная A/A-калибровка, [протокол](docs/performance.md) |

Criterion A/A–A/B пока находится в [PR #8](https://github.com/qqrm/openfga-axum-lab/pull/8).
Не путайте Criterion mean, индивидуальные HTTP p50/p95/p99 и ёмкость сервиса.
Бенчмарки и chaos-тесты никогда не запускаются на открытой ручной лаборатории.

| Требование стенда | Реализация | Проверка |
| --- | --- | --- |
| Объектная авторизация | HTTP/use cases + FGA model | `system`, `lab-tool acceptance` |
| Отзыв между репликами | `cached_authorizer.rs` | `cache_replicas` |
| Восстановление и идемпотентность | lifecycle + PostgreSQL | `system`, Kani для ограниченного ядра |
| Установка и изоляция | image-only Compose | `compose_deployment`, `delivery` |
| Проверенный выпуск | Rust delivery policy | unit-тесты и release workflows |
| Закрытая временная лаборатория | private Codespaces port, email tunnel | `live_lab` negative controls, devcontainer smoke |

Это согласованные цели стенда, не реконструкция отсутствующего исходного
задания заказчика. [Архитектура](docs/architecture.md),
[проверки и их границы](docs/verification.md), [INTENT](INTENT.md).

`POST/DELETE /session` создаёт/отзывает bearer-сессию. API:
`GET/POST /pets`, `GET/PATCH/DELETE /pets/{id}`,
`PUT/DELETE /pets/{id}/relations/{viewer|caregiver}/{user}`.
Создание требует `Idempotency-Key`; конфликт содержимого — 409.
OpenFGA errors закрывают доступ с 503, а не становятся allow. `/metrics` — Prometheus.

Не выставляйте исходный Compose в интернет. Постоянный публичный сервис требует
собственных секретов, TLS, ограничения доступа и backup. Actions-лаборатория —
не круглосуточный хостинг, не автоматически продлевается и не хранит данные
между сеансами. Изменения идут через рабочую ветку и PR, без прямых push в main.

## Инженерия и проверки

[INTENT.md](INTENT.md) фиксирует цель, [AGENTS.md](AGENTS.md) — инструкции.
Доменное ядро использует чистые функции и явные типы; Axum служит HTTP-слоем,
а сценарии и интеграции вынесены отдельно. Свойства жизненного цикла проверены
Kani в ограниченной модели. Прохождение модели не является доказательством
всего распределённого сервиса.

CI включает форматирование, Clippy, nextest, cargo-audit/deny/machete,
ShellCheck, actionlint, zizmor, Kani и контейнерные сценарии с отказами.
Намеренно ошибочные примеры проверяют чувствительность ряда инструментов.
Rust-тест контракта запрещает возврат Python-исходников и Python-вызовов в
workflows/Compose и проверяет наличие интеграционных наборов в ручном CI.
Таблица покрытия и пробелов: [проверки](docs/verification.md).

GitHub Dependabot использует расписание из пользовательского шаблона.
Автослияние дополнительно проверяет защиту основной ветки и не включается,
если требуемая защита отсутствует. Проверка подписей и авторства коммитов
реализована в `lab-tool dependency-policy`; workflow с правом записи по-прежнему
не извлекает и не выполняет код PR. CodeQL анализирует публичный репозиторий.

В Cargo.lock присутствует необязательная MySQL-зависимость `rsa` из SQLx с
RUSTSEC-2023-0071. Аудит допускает только это предупреждение и сначала требует,
чтобы `rsa` отсутствовала в разрешённом графе для всех целей; появление
зависимости в графе блокирует проверку.

## Производительность и браузер

Интерактивная версия в `browser-demo/` работает целиком в браузере: выбор
роли и данные имитируются, а жизненный цикл использует то же доменное ядро
Rust, собранное в WebAssembly. Настоящие Axum, OpenFGA и PostgreSQL в этой
версии не запускаются. Для локальной сборки установите цель Rust
`wasm32-unknown-unknown` и выполните `bash scripts/build-browser-demo.sh`,
затем раздайте каталог `browser-output` любым HTTP-сервером. Ручной workflow
`Browser preview` публикует этот вариант на GitHub Pages.

Rust-тест `diagnostic_latency` фиксирует сырые задержки, ошибки и окружение.
Отдельный ручной workflow выполняет рандомизированную A/A-калибровку.
На виртуальном runner это диагностика, а не доказательство регрессии или
ёмкости системы: [протокол](docs/performance.md).

[Варианты запуска в браузере](docs/browser-feasibility.md) рассматриваются
отдельно. Compose-стенд, эмуляция контейнеров и статическая демонстрация имеют
разные гарантии. Настоящая контейнерная лаборатория проверяется отдельно
workflow `Browser lab verification`.

## Just-команды

Установите [just](https://github.com/casey/just), затем:

```sh
just build
just check
just start
just logs
just test
just stop
```

Необязательный Playwright-набор (`just e2e`) использует экспортированный
`DEMO_PASSWORD` работающего локального стенда. Текущий CI этот набор не запускает.

Английская версия: [README.en.md](README.en.md).

## Kubernetes: экспериментальный путь

Манифесты в `k8s/` используют тот же образ приложения и Rust `lab-tool` для
инициализации. Сборка, применение в кластере и проверка сервиса в Kubernetes
не входят в подтверждённые CI-проверки; этот путь пока не проверен в кластере.
Для воспроизводимого стенда используйте описанный выше Compose или Codespaces.

## OpenFGA transport

The default `grpc` feature uses OpenFGA's native gRPC port 8081. The optional
`http` feature uses port 8080. Bootstrap and test fixture inspection always use
HTTP, independently of the application's selected transport. If both features
are enabled, the application selects gRPC.

```sh
cargo build --locked
cargo build --locked --no-default-features --features http
```

Both adapters request `HIGHER_CONSISTENCY`, tolerate duplicate grants and
missing revocations, and bound authorization requests to two seconds. CI runs
the real authorization, cache race, restart, recovery and Docker deployment
suites separately for each transport. `/readyz` independently verifies PostgreSQL,
the selected transport, the store and the configured authorization model. It
never relies on an authorization cache hit.

Compose builds gRPC by default. To build and run the HTTP variant, set both the
build feature and the application's endpoint (bootstrap keeps using HTTP):

```sh
FGA_TRANSPORT=http FGA_APP_URL=http://openfga:8080 docker compose up --build
```

The image-only `compose.release.yaml` expects a gRPC image by default. If you
explicitly build an HTTP image, also set `FGA_APP_URL=http://openfga:8080` for
that environment. Bootstrap continues to use HTTP in either case.

## Backend diagnostic benchmark

`backend_bench` requires `FGA_URL`, `FGA_STORE` and `FGA_MODEL` for an existing
isolated lab. `BACKEND` defaults to the enabled transport; `ITERS` defaults to
1000 and must be positive. Any failed or denied authorization, including during
warmup, aborts without publishing timings.

```sh
cargo run --locked --release --bin backend_bench
cargo run --locked --release --no-default-features --features http --bin backend_bench
```

The previous gRPC/HTTP timing table was invalid: the adapters used different
consistency settings and the runner counted failed checks as samples. Those
numbers do not establish a performance difference. The current tool is a
single-threaded diagnostic, not a capacity or SLO measurement; use repeated,
paired runs with correctness and noise checks before making performance claims.
