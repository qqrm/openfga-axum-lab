# Лапки — Rust/Axum и объектная авторизация OpenFGA

Небольшой инженерный стенд: приют, роли и разрешения на отдельные объекты,
PostgreSQL, OpenFGA, отзыв доступа между репликами и восстановление после отказов.
Данные вымышлены. Это лабораторная демонстрация, не готовая публичная IAM-система.

## Что можно открыть и запустить

| Вариант | Что работает | Состояние |
| --- | --- | --- |
| [Браузерное демо](https://qqrm.github.io/openfga-axum-lab/) | WASM-ядро; данные и права имитируются | Отдельная статическая публикация, не backend |
| Локальный Compose | Настоящие Axum, PostgreSQL и OpenFGA | Запуск ниже; проверяется в CI |
| Контейнерная поставка | Тот же стек из готового образа без сборки Rust | Подготовлены image-only Compose и release workflow; опубликованный релиз не заявляется до его появления в Releases |
| Staging / stable | Независимые постоянные backend-окружения | Хост и HTTPS-адреса ещё не настроены; одноразовые CI-стенды ими не считаются |

## Быстрый запуск из исходников

Нужны Git, Docker Engine и Docker Compose v2. Команды ниже — для Linux/WSL2.
Rust на хосте для этого запуска не нужен:

```sh
git clone https://github.com/qqrm/openfga-axum-lab.git
cd openfga-axum-lab
docker compose up -d --build app
```

Откройте `http://localhost:3000`. Войдите как `alice`, `bob`, `carol` или
`dave`; пароль локального исходного стенда — `local-demo-password`.
`alice` — администратор, `bob` — сотрудник, `carol` — владелец лисы Mika,
`dave` изначально видит кота Pixel, но не лису.

Compose создаёт две базы в одном PostgreSQL. `lab-tool bootstrap` публикует
модель и исходные отношения OpenFGA; повторный запуск сохраняет store/model,
а не молча заменяет их. `APP_PORT=3001 docker compose up -d app` меняет порт.
`DEMO_PASSWORD` задаёт только первоначальный пароль: существующие аккаунты
после инициализации этой переменной не обновляются.

## Проверить основную задачу

Готовая Rust-команда проверяет последовательность HTTP-статусов
**403 → 204 → 200 → 204 → 403**: `dave` получает отказ на лису,
`carol` выдаёт доступ, чтение проходит, доступ отзывается, чтение запрещено.
Она возвращает исходные права и отзывает созданные тестовые сессии.
Запускайте на выделенном демонстрационном стенде, а не на чужих данных:

```sh
docker compose exec -e SMOKE_ALLOW_MUTATIONS=true app lab-tool acceptance
```

Успех — JSON с `accepted: true`, ожидаемыми статусами и версией приложения.
Уже выданный `dave` доступ считается неожиданным состоянием: команда не
стирает его перед проверкой. Любая ошибка даёт ненулевой exit code.

Диагностика:

```sh
docker compose ps
docker compose logs --tail=100 app bootstrap openfga postgres
curl --fail http://localhost:3000/healthz
curl --fail http://localhost:3000/readyz
curl --fail http://localhost:3000/version
```

`/healthz` — процесс жив. `/readyz` — доступны PostgreSQL, OpenFGA и конкретные
store/model; при отказе зависимости вернётся 503. `/version` показывает версию
пакета и встроенный source SHA; у непомеченной локальной сборки SHA — `unknown`.
Не используйте liveness как доказательство готовности авторизации.

`docker compose down` сохраняет данные. **Только для полного сброса своего
локального стенда** используйте `docker compose down -v`: это удаляет все его
тома. Не выполняйте эту команду на staging/stable.

## Запуск готовой поставки без Rust

Порядок скачивания проверенного bundle, проверки SHA-256, генерации разных
секретов и запуска `compose.release.yaml`: [доставка и окружения](docs/delivery.md).
Образ выбирается по digest, не по `latest`. В поставке есть приложение,
`lab-tool`, миграции и модель; в bundle — Compose и инициализация PostgreSQL.
Неполный нативный архив больше не является основным форматом выпуска.

## Тесты и результаты

Для разработки нужны Rust из `rust-toolchain.toml`, Docker/Compose и nextest:

```sh
cargo install cargo-nextest --locked
cargo nextest run --locked --profile ci
# Все обычные сценарии, включая Docker (без отдельной A/A-калибровки):
cargo nextest run --locked --release --profile ci --run-ignored all \
  -E 'not test(paired_aa_calibration) and not binary(delivery)' --success-output immediate
```

Проверка image-only поставки на двух одноразовых стендах:

```sh
docker build --build-arg APP_REVISION="$(git rev-parse HEAD)" -t petlab-candidate:local .
export DELIVERY_IMAGE="$(docker image inspect petlab-candidate:local --format '{{.Id}}')"
cargo nextest run --locked --release --profile ci --test delivery --run-ignored only --success-output immediate
```

Тест проверяет разные секреты, stores, тома и сети, отказ чужой сессии,
выдачу/отзыв разрешений, перезапуск, согласованное восстановление двух БД,
отказ OpenFGA и удаление store. Его `down -v` ограничен случайными test-owned
Compose projects, не обычным локальным стендом.

В GitHub Actions выберите workflow → **Run workflow** → ветку; права записи
нужны только для ручного запуска. PR запускает проверки автоматически.

| Workflow | Назначение и результат |
| --- | --- |
| [CI](https://github.com/qqrm/openfga-axum-lab/actions/workflows/ci.yml) | fmt, Clippy, nextest, реальные интеграции, Kani и зависимости; JUnit в артефактах |
| [Delivery verification](https://github.com/qqrm/openfga-axum-lab/actions/workflows/delivery.yml) | Реальный собранный контейнер на двух одноразовых стендах; `delivery-evidence.json` |
| [Workflow Security](https://github.com/qqrm/openfga-axum-lab/actions/workflows/workflow-security.yml) | actionlint, zizmor и тесты политики |
| [Secret scan](https://github.com/qqrm/openfga-axum-lab/actions/workflows/secrets.yml) / [CodeQL](https://github.com/qqrm/openfga-axum-lab/actions/workflows/codeql.yml) | Проверки исходников и секретов |
| [Manual performance calibration](https://github.com/qqrm/openfga-axum-lab/actions/workflows/performance.yml) | Отдельная A/A-калибровка; сырые данные и ограничения в [протоколе](docs/performance.md) |
| [Browser preview](https://github.com/qqrm/openfga-axum-lab/actions/workflows/pages.yml) | Ручная публикация статического демо, не backend deployment |

Criterion A/A–A/B и результаты его выполненного эксперимента находятся в
[отдельном PR #8](https://github.com/qqrm/openfga-axum-lab/pull/8), пока он не
принят. Не путайте Criterion mean, индивидуальные HTTP p50/p95/p99 и максимальную
ёмкость сервиса. Независимый Goal #10 не меняет методику этих измерений.

## Что где проверяется

| Условие | Реализация | Проверка |
| --- | --- | --- |
| Реальная объектная авторизация | `http.rs`, `use_cases.rs`, OpenFGA model | `system`, `lab-tool acceptance` |
| Отзыв между репликами / неизвестный результат записи | `cached_authorizer.rs` | `cache_replicas` |
| Идемпотентность и восстановление | pending/deleting lifecycle, PostgreSQL | `system`, Kani только для ограниченного доменного ядра |
| Запуск исходного Compose | Dockerfile + Rust bootstrap | `compose_deployment` |
| Готовая поставка и изоляция окружений | `compose.release.yaml`, `tools/delivery.rs` | `delivery` |
| Не выпускать непроверенный SHA / другой digest | `tools/delivery_policy.rs` | Unit-тесты политики, проверки release/deploy workflows |

Это матрица согласованных целей стенда, не реконструкция отсутствующего
исходного задания заказчика. Подробности: [архитектура](docs/architecture.md),
[проверки и границы доказательств](docs/verification.md), [INTENT](INTENT.md).

## API и ограничения

`POST /session` выдаёт ограниченный по времени bearer-токен; `DELETE /session`
его отзывает. Основные операции: `GET/POST /pets`, `GET/PATCH/DELETE /pets/{id}`,
`PUT/DELETE /pets/{id}/relations/{viewer|caregiver}/{user}`. Создание требует
`Idempotency-Key`; повтор с другим содержимым даёт 409. `/metrics` — Prometheus.
OpenFGA errors закрывают доступ, а не превращаются в allow.

Локальные demo-аккаунты, внутренние соединения без TLS и единый служебный
DB-пользователь — границы лаборатории. Не выставляйте исходный Compose в
интернет. Постоянные стенды требуют собственных секретов, ограниченного
доступа, TLS, backup и проверенного восстановления. Новые файловые секреты не
делают локальный Docker Engine безопасным от его администраторов.

Рабочий процесс: **feature → PR → проверки → main → staging → подтверждённый
release → stable**. Постоянная `dev` и Kubernetes для этого не нужны.
Benchmarks/chaos используют отдельные одноразовые стенды.
