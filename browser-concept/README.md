# Permission Studio: реальная авторизация внутри браузера

Самостоятельный небольшой сценарий: Alice владеет документом, Bob получает и
теряет право чтения. Страница запускает Linux через QEMU/Wasm; внутри работают
настоящие PostgreSQL, OpenFGA и новый Rust/Axum executable `concept`.
Решения и HTTP-ответы приходят от этих процессов.

**Проверенный вариант на 20 ноября 2026 — desktop Chrome.**
[Chromium job](https://github.com/qqrm/openfga-axum-lab/actions/runs/44688401944/job/136129171047)
полностью прошёл два цикла выдачи/отзыва доступа и шесть проверок запрета Bob.
Готовый [ZIP стенда](https://github.com/qqrm/openfga-axum-lab/actions/runs/44688401944/artifacts/14608585369)
проверен по SHA-256 и всем файлам manifest; после распаковки выполните `node serve.mjs`.
Firefox 159 в этом же run не дошёл до гостевого сценария за 20 минут, поэтому
его поддержка этим результатом не подтверждена, а общий workflow остался красным.
Подробности и исходные логи — в [отчёте проверки](../docs/browser-concept-check-20261120.md).

Этот сценарий использует собственную модель и собственные данные. Полный стенд
с моделью Lakekeeper и его acceptance остаются отдельными:
[основной стенд](../README.md), [полный браузерный сценарий](../browser-single/README.md).
Успешная сборка концепта или его native-проверка ещё не означают прохождение
браузерного Check. Его результат устанавливает соответствующий CI job.

## Архитектура

| Часть | Роль |
| --- | --- |
| [Страница](pages/index.html) и [launcher](pages/launcher.js) | Запускают VM по кнопке, отправляют команды и отображают реальные HTTP-статусы, тело документа и последние наблюдения прав |
| [`concept bridge`](../src/bin/browser_concept/bridge.rs) | Читает команды из виртуального serial-порта, выбирает отдельную demo-сессию Alice или Bob и делает HTTP-запрос к Axum внутри гостя |
| [`concept serve`](../src/bin/browser_concept/api.rs) | Определяет пользователя по bearer-сессии в PostgreSQL, авторизует запрос через общий HTTP-адаптер OpenFGA и выполняет операцию |
| PostgreSQL | Хранит один документ и две сессии в отдельной схеме `browser_concept` |
| OpenFGA | Хранит модель и кортежи в памяти; вычисляет `can_read` и `can_share`, принимает выдачу и отзыв `viewer` |

Rust-приложение собирается как Linux x86-64 ELF с исходными `Cargo.toml` и
`Cargo.lock`. OpenFGA — закреплённый ELF сборки `v1.10.3-browser-startup.10`.
При старте эта сохранённая сборка OpenFGA требует исходную Lakekeeper-фикстуру
для прогрева JSON-кодеков. Файл передаётся только процессу OpenFGA и не становится
активной моделью концепта: затем Rust создаёт отдельный store и загружает туда
двухтипную модель ниже. Native proof отдельно проверяет обе идентичности.
QEMU/Wasm исполняет оба внутри Linux с **двумя vCPU и 512 MiB гостевой памяти**.
Расход памяти самого браузера может быть существенно выше. Браузеру не нужен
внешний API-сервер: после загрузки статических файлов запросы проходят внутри VM.

Исходный rootfs, kernel и emulator не пересобираются. Сборщик проверяет сохранённые
артефакты и добавляет новый executable, guest entrypoint, модель и UI.
Используются следующие независимые источники:

| Слой | Закреплённый источник |
| --- | --- |
| Исходный rootfs | `99f00cc170ca611ff4dc4c4ae3bbe98bb10cff58` |
| Проверенный runtime и browser helpers | `444b069daf16e9606a0bbf1ea0aea7f4d61b97d8` |
| Emulator | `6f35f24b7ca345cf47c873becee6a41ef94e65a9` |
| Новый Rust executable, модель и UI | Точный commit текущего concept workflow |

Идентичности producer jobs, размеры и SHA-256 находятся в
[`fetch-runtime-inputs.mjs`](fetch-runtime-inputs.mjs),
[`retained/provenance.json`](retained/provenance.json) и собранном `provenance/`.
Просроченные исходные архивы допускают только заранее закреплённую и проверенную
зеркальную копию тех же байтов; другой runtime автоматически не подбирается.

## Вся модель

Исполняемый источник — [`model.json`](model.json), встроенный в Rust binary.
Читаемая форма — [`model.fga`](model.fga):

```fga
model
  schema 1.1

type user

type document
  relations
    define owner: [user]
    define viewer: [user]
    define can_read: owner or viewer
    define can_share: owner
```

Начальный кортеж: `user:alice` — `owner` — `document:demo`.
Выдача доступа добавляет `user:bob` — `viewer` — `document:demo`, отзыв удаляет
этот кортеж. Оба изменения требуют отдельного успешного `can_share` для текущей
сессии. Поэтому Bob может получить чтение, но не право делиться документом.
Тело запроса не определяет действующего пользователя.

| HTTP-операция | Проверка / результат |
| --- | --- |
| `GET /concept/health` | Доступность PostgreSQL/OpenFGA, идентичность модели и созданного store |
| `GET /concept/permissions` | Реальные `can_read` и `can_share` текущей сессии |
| `GET /concept/document` | `can_read`; документ из PostgreSQL при разрешении, иначе `403` |
| `PUT /concept/document/viewer` | `can_share`, затем запись `viewer` для Bob; успех `204` |
| `DELETE /concept/document/viewer` | `can_share`, затем удаление `viewer` для Bob; успех `204` |

Для двух операций изменения принимается только `{"user":"bob"}`.
Отсутствующая или неверная сессия даёт `401`, некорректный запрос — `400`,
ошибка backend — `503`. Неопределённый исход не превращается в разрешение;
UI не повторяет запись после таймаута.

## Сценарий в UI

1. Нажмите **Start the playground**. До этой кнопки страница читает metadata,
   но не загружает большой VM runtime. Объём загрузки показывается заранее.
2. Дождитесь готовности. Выбран Bob: **Read as Bob** должен вернуть `403`.
3. Выберите Alice и нажмите **Grant access**: `204`. UI заново запрашивает права
   и чтение Bob; документ становится доступен, чтение возвращает `200`.
4. Выберите Bob и попробуйте **Grant access** или **Revoke access**: `403`,
   даже когда Bob уже viewer.
5. Выберите Alice и нажмите **Revoke access**: `204`. Повторное чтение Bob
   возвращает `403`. Цикл можно повторить.

Плашки прав показывают последние завершённые проверки сервера. История содержит
идентификатор запроса, пользователя, операцию, фактический HTTP-статус и время.
**Runtime & diagnostics** открывает provenance и ограниченный видимый guest log;
CI отдельно сохраняет полный поток.

## Сборка и запуск

Все команды ниже выполняются из корня репозитория. Для локальной проверки нового
Rust binary достаточно toolchain из `rust-toolchain.toml`:

```sh
rustfmt --edition 2024 --check src/bin/browser-concept.rs
cargo clippy --locked --bin browser-concept -- -D warnings
cargo test --locked --bin browser-concept
cargo build --locked --release --bin browser-concept
```

Для Linux executable, собранного и проверенного в Bookworm, нужен Docker Buildx:

```sh
docker buildx build --platform linux/amd64 \
  --file browser-concept/Dockerfile --target output \
  --output type=local,dest=artifacts/browser-concept-local .
```

Результат — `concept` и логи format/lint/unit/build в указанной директории.
Эта команда не создаёт браузерный bundle и не выдаёт доказательство Actions run.
В CI [`build-native.sh`](build-native.sh) дополнительно закрепляет digest образа
`rust:1.99.1-slim-bookworm`, исходники, SHA-256 executable/model и настоящую
идентичность producer job. Скрипт требует чистый commit и реальные `GITHUB_*`
переменные; для локального запуска их подделывать не нужно.

### Workflow

[`Browser concept with real PostgreSQL and OpenFGA`](../.github/workflows/browser-concept.yml)
запускается push-ем изменений перечисленных в workflow путей в ветку
`feat/browser-concept-20261120` репозитория `qqrm/openfga-axum-lab`.
`workflow_dispatch` отсутствует; изменение только README новый VM run не запускает.

Последовательность фиксирована:

1. **Build browser concept binary** — shell/protocol preflight, format, Clippy,
   unit tests и сборка только нового binary; сохраняется `browser-concept-native`.
2. **Browser concept (chromium)** — восстановление проверенных VM inputs,
   native integration того же ELF с PostgreSQL 17 и OpenFGA, сборка сайта,
   sandbox smoke системного Chrome и одна свежая VM с действиями через UI.
3. **Browser concept (firefox)** — только после успешного Chromium job;
   получает его точный bundle и запускает одну новую VM с тем же сценарием.

Native job ограничен 20 минутами, каждый browser job — 35 минутами.
Старт VM разрешён не позднее 10 минут от фактического начала текущего job;
полная браузерная проверка ограничена 20 минутами внутри 22-минутного шага.
Автоматических повторов VM и матрицы параметров нет; workflow принимает только
первую попытку run. Для обоих браузеров используется собственное время job из API.

Успешный Chromium job сохраняет готовый сайт как **`browser-concept-site-chromium`**.
После скачивания и распаковки ZIP в отдельную директорию его можно открыть локально:

```sh
node /path/to/unpacked-concept/serve.mjs
```

Откройте `http://127.0.0.1:8765/`; `PORT` меняет порт. Из исходного репозитория
тот же сервер запускается как `node browser-concept/serve.mjs /path/to/unpacked-concept`.
Сервер раздаёт только статические файлы и слушает только loopback.
Нужны desktop browser, JavaScript,
`SharedArrayBuffer` и cross-origin isolation; scoped service worker добавляет
нужные заголовки и может один раз перезагрузить страницу. Для размещения вне
localhost требуется HTTPS. Сам workflow сайт не публикует.

## Что подтверждают результаты

| Проверка | Область доказательства |
| --- | --- |
| Native build | Исходники и модель привязаны к ELF; format, Clippy и unit tests выполнены; допустимая версия glibc для сохранённого гостя |
| [`check-native.mjs`](check-native.mjs) | Настоящие PostgreSQL/OpenFGA/Axum и Rust bridge; два цикла `403 → 204 → 200 → 204 → 403`, отказ Bob в изменениях, `401`, неправильные тела и `503` после остановки OpenFGA |
| [`check-browser.mjs`](check-browser.mjs) | Тот же проверенный bundle, реальная готовность гостя и двух CPU, два цикла через кнопки UI, отказы Bob, соответствие видимых ответов и состояния; полный захват логов и отсутствие page errors |

Native результат хранится в `artifacts/browser-concept-native-proof/result.json`,
browser результаты — в `artifacts/browser-concept-browser/<engine>/result.json`.
Артефакты `browser-concept-chromium-evidence` и `browser-concept-firefox-evidence`
сохраняют результаты, логи, скриншоты и provenance. Для вывода об успехе нужен
`passed: true` соответствующего результата и успешный job, а не только сборка
или сообщение о загрузке Linux.

Фактические результаты запусков, измерения и причины отказов записаны в
[наблюдениях browser Check](../docs/browser-concept-check-20261120.md).

Концепт не запускает приложение полного стенда, его миграцию, матрицу Lakekeeper
или `lab-tool acceptance`. Даже успешные оба browser jobs подтверждают только
этот маленький сценарий; поля `lakekeeperAcceptance` и `productionAcceptance`
остаются `false`. Сохранённые исходные VM layers не означают повторную проверку
всех процессов, которые когда-либо входили в rootfs.

## Временные данные

Каждая VM создаёт свежие PostgreSQL cluster/database, OpenFGA store/model,
один вымышленный документ и две независимые demo-сессии. Сырые bearer-токены
остаются в приватном файле внутри гостя; PostgreSQL хранит их хеши. Выбор Alice
или Bob — переключение тестовой сессии, а не форма входа для реальных пользователей.

**Reset this tab**, перезагрузка и закрытие страницы теряют эту VM и изменения
прав. При следующем старте Bob снова не имеет доступа. Данные не сохраняются в
IndexedDB или на внешнем backend. Native checker создаёт отдельный временный
PostgreSQL container и удаляет свои данные после проверки. Сам Rust server
отказывается переиспользовать существующую схему или credentials-файл, вместо
того чтобы очищать чужие данные.
