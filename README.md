# Rust / Axum / OpenFGA — Authorization Lab

Нативная интеграция объектной авторизации в Rust: реальные проверки OpenFGA,
роли, владельцы, иерархии, списки с фильтрацией и перенос существующих SQL-прав.
При неопределённом результате система закрывает доступ.

Используется **полная неизменённая модель Lakekeeper v4.15** — публичного
Rust-каталога Apache Iceberg. Исходный файл закреплён на коммите
[`b73d5f8083a690ca4f533559e6ef3cbc6fbc13d6`](https://github.com/lakekeeper/lakekeeper/blob/b73d5f8083a690ca4f533559e6ef3cbc6fbc13d6/authz/openfga/v4.15/schema.json).
Это самостоятельный стенд с подмножеством операций этой модели; пользователи и
данные вымышлены. Он не запускает Lakekeeper и не воспроизводит его SQL-схему.
Источник, SHA-256, Apache-2.0 LICENSE и NOTICE находятся в [`fga/`](fga/).

[Личная лаборатория в Codespaces](https://codespaces.new/qqrm/openfga-axum-lab?quickstart=1) ·
[Статический обозреватель модели](https://qqrm.github.io/openfga-axum-lab/) ·
[English overview](README.en.md)

## Run the live lab

Для нового изолированного стенда нужны Docker Engine, Compose v2 и OpenSSL.
Rust на хосте не нужен.

```sh
git clone https://github.com/qqrm/openfga-axum-lab.git
cd openfga-axum-lab
export POSTGRES_PASSWORD="$(openssl rand -hex 24)"
export DEMO_PASSWORD="$(openssl rand -hex 24)"
docker compose -p authzlab-catalog up -d --build app
```

Откройте `http://127.0.0.1:3000`. Пароль — значение `DEMO_PASSWORD` из вашей
оболочки. Сохраните эти переменные для последующих команд Compose; не меняйте
пароль существующего PostgreSQL-тома повторной генерацией. `APP_PORT` меняет порт.

| Пользователь | Исходные права в синтетических данных |
| --- | --- |
| `alice` | Администратор проекта `acme`, владелец хранилища, пространств имён и ролей |
| `bob` | Участник `analysts`; чтение через вложенную роль, создание таблиц; владелец `errors` |
| `carol` | Владелец таблицы `requests` |
| `dave` | Прямое `select` на `latency` |
| `eve` | Только другой проект `other`; одноимённая `requests` в хранилище `external` |

Консоль показывает фактические ответы API. Можно проверять действия текущего
пользователя, выдавать и отзывать права, назначать роль, менять владельца и
переносить таблицу между пространствами имён. Операции повторно авторизуются в
сервисном слое. Субъект проверки берётся из сессии, не из пользовательского JSON.

Локальный Compose явно включает `DEMO_SEED=true` и `DEMO_MIGRATE=true`: создаёт
синтетические SQL-данные, затем выполняет настоящую миграцию. Без этих флагов
приложение не заполняет данные и не активирует авторизацию автоматически.

```sh
curl --fail http://127.0.0.1:3000/readyz
docker compose -p authzlab-catalog exec -e SMOKE_ALLOW_MUTATIONS=true app lab-tool acceptance
docker compose -p authzlab-catalog down
```

Acceptance выполняет **403 → 204 → 200 → 204 → 403**: запрет для Dave, выдача
права владельцем, успешное чтение, отзыв, повторный запрет. Используйте его на
изолированной исходной фикстуре, без параллельного ручного изменения этих прав.
`down` сохраняет тома. Удаление томов не является шагом обновления.

### Обновление прежнего стенда

Существующие SQL-миграции сохранены без переписывания. Новый каталог использует
новые таблицы и имя БД `authzlab`; прежние предметные данные автоматически не
переименовываются. Старый fingerprint OpenFGA отклоняется с требованием явной
миграции. Для нового демо используйте отдельный Compose project, как выше, и
сохраните прежние тома. Полный перенос произвольной старой предметной области не
входит в миграцию каталога.

## Что проверяет авторизация

Все пути таблиц начинаются с
`/projects/{project}/warehouses/{warehouse}/tables`.

| HTTP-операция | Проверка в закреплённой модели |
| --- | --- |
| `GET /…/tables` | Ограниченная выборка SQL + `can_get_metadata` для каждой возвращаемой таблицы |
| `GET /…/tables/{id}` | `can_get_metadata` на `lakekeeper_table:{warehouse}/{id}` |
| `POST /…/tables` | Права создания/доступа к выбранному namespace; `Idempotency-Key` обязателен |
| `PATCH /…/tables/{id}` | `can_commit`; при переименовании также `can_rename` |
| `DELETE /…/tables/{id}` | `can_drop`; удаляются прямые кортежи и обратная связь родителя |
| `PUT/DELETE /…/tables/{id}/grants/{privilege}` | Отдельные `can_grant_*` / `can_revoke_*` |
| `PUT /…/tables/{id}/owner` | `can_change_ownership`; новый владелец обязан состоять в проекте |
| `PUT /…/tables/{id}/namespace` | Право переноса источника и необходимые права приёма в назначении |
| `POST /…/tables/{id}/check` | Только разрешённое действие и субъект текущей сессии |
| `PUT/DELETE /projects/{project}/roles/{role}/assignees/{user}` | Управление assignee; роль и пользователь из того же проекта |

`POST/DELETE /session` создаёт/отзывает непрозрачную bearer-сессию.
`/catalog` возвращает доступную директорию, `/model` — provenance,
`/model/schema` — исходную модель. `/healthz` показывает жизнь процесса;
`/readyz` проверяет связанный с target статус миграции, fence незавершённых
изменений и настоящий backend. `/metrics` содержит ограниченные метки без токенов.

Пример выдачи права:

```json
{"subject":{"type":"role","id":"analysts"}}
```

Допустимые privileges: `describe`, `select`, `write`, `modify`, `pass_grants`,
`manage_grants`. `ownership` меняется отдельной операцией. Неизвестные поля JSON,
неизвестные действия, неоднозначные заголовки и некорректные ID отклоняются.

## Перенос ролей, владельцев и связей

`lab-tool bootstrap` создаёт **пустой** store и закреплённую модель; он не
импортирует JSON-фикстуру кортежей. `lab-tool migrate-authorization` читает
реляционные таблицы каталога, legacy ACL и назначения ролей.

Миграция проверяет принадлежность проектам, ссылки, допустимые privileges и
циклы ролей до первой записи, закрепляет source hash и store/model/hash,
записывает идемпотентные batch-и с checkpoint, затем сверяет полный ожидаемый
набор кортежей и матрицу прав с независимым legacy evaluator. До успешной сверки
статус не становится `active`; чтения и изменения каталога возвращают 503.
Перезапуск продолжает checkpoint. Повторный запуск после активации не
восстанавливает уже отозванные legacy-права.

Команды, формат SQL-контракта, ограничения и сценарии cutover:
[миграция](docs/migration.md).

## Fail-closed и AppSec

- Каждый `allow` требует нового успешного `Check` с `HIGHER_CONSISTENCY`.
  Кэшируются только отрицательные решения; health/readiness не заменяет Check.
- Таймаут, неверный HTTP-статус, редирект, неоднозначный JSON, неверный bool,
  неподтверждённая запись и превышение лимитов не превращаются в успех.
- Перед записью графа PostgreSQL сохраняет общий для реплик fence. Если исход
  записи неизвестен, все решения и readiness остаются закрытыми, в том числе
  после перезапуска. Успешная следующая запись не снимает чужой fence.
- Проект, хранилище, ID и субъект связаны типами и проверками SQL. Сервис повторно
  проверяет права под блокировками проекта и объекта; middleware не является
  единственным местом проверки.
- Смена владельца и родителя сохраняет durable intent и применяет обе стороны
  изменения одним OpenFGA Write. До завершения объект не виден.
- Списки имеют лимиты размера, числа кандидатов и параллельных Check.
  Случайный cursor связан с субъектом и scope, имеет срок действия.

Подробности и оставшиеся границы: [AppSec](docs/appsec.md),
[архитектура](docs/architecture.md). Это ограниченная лаборатория, не доказанная
распределённая транзакция и не готовый публичный IAM-сервис.

## Проверки

```sh
cargo nextest run --locked --all-features --profile ci
cargo nextest run --locked --release --profile ci \
  --no-default-features --features http \
  --test system --test cache_replicas --test authorization_migration \
  --test compose_deployment --test performance \
  --run-ignored only -E 'not test(paired_aa_calibration)' --success-output immediate
```

Вторую команду повторяют с `--features grpc`. Docker-тесты имеют `#[ignore]`,
поэтому обычный unit-прогон их не подтверждает. CI явно запускает обе матрицы.
Настоящие браузерные E2E также входят в `ci-gate`:

```sh
(cd e2e && npm ci --ignore-scripts && npx --no-install playwright install chromium)
export DEMO_PASSWORD="$(openssl rand -hex 24)"
bash scripts/browser-e2e.sh
```

Скрипт создаёт и удаляет только собственный одноразовый стенд; не запускайте его
для диагностики живой пользовательской среды. Тесты и их границы перечислены в
[verification](docs/verification.md). Дополнительные workflows проверяют
поставку на двух стендах и dump/restore, devcontainer, зависимости, секреты,
workflow security и ограниченную модель lifecycle Kani.

## Транспорт, браузер и поставка

По умолчанию используется **HTTP** (`reqwest` + `serde`, порт 8080). gRPC
остаётся дополнительным адаптером. Если обе Cargo features включены, бинарник
выбирает gRPC. Для Compose gRPC нужны оба параметра:

```sh
FGA_TRANSPORT=grpc FGA_APP_URL=http://openfga:8081 \
  docker compose -p authzlab-catalog-grpc up -d --build app
```

Bootstrap и SQL-миграция используют HTTP независимо от runtime-транспорта.
`FGA_HTTP_URL` задаёт этот endpoint для явного demo cutover.

Codespaces запускает настоящий приватный стенд; пароль находится в
`.lab/codespaces/secrets/demo-password`. Pages показывает исходную модель и
WASM lifecycle, без выдуманных авторизационных решений. Временный защищённый
Actions-стенд запускается только вручную: [live lab](docs/live-lab.md).

[Поставка](docs/delivery.md) описывает образы по digest, раздельные среды,
backup/restore и ограничения отката. Экспериментальные Kubernetes-манифесты
не проверены в кластере. [Протокол производительности](docs/performance.md)
отделяет диагностические задержки от утверждений об SLO или пропускной
способности. Исходный Compose рассчитан на изолированный локальный запуск.
