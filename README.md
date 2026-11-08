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

В Codespace установлен Rust 1.98; для фиксированной версии используйте
`cargo +1.98.0`. Nextest устанавливается приведённой командой, не скрытым
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
