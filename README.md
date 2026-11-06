# Лапки — приют и объектная авторизация

Небольшой pet-проект с реальным Rust/Axum API, PostgreSQL и OpenFGA. Животные,
учётные записи и данные вымышлены. Проект показывает инженерный подход к
авторизации и отказам; это не готовая к публичной эксплуатации система.

[Открыть интерактивную демонстрацию](https://qqrm.github.io/openfga-axum-lab/)
— в браузере права и данные имитируются. Полный сервис с реальным OpenFGA
запускается через Compose по инструкции ниже.

## Запуск

Нужны Docker и Docker Compose:

```sh
git clone --recurse-submodules https://github.com/qqrm/openfga-axum-lab.git
cd openfga-axum-lab
docker compose up -d --build
docker compose --profile test run --rm test
```

Откройте [http://localhost:3000](http://localhost:3000). Compose создаёт две
отдельные базы в одном PostgreSQL: данные приюта и хранилище OpenFGA.
`docker compose down` сохраняет тома, а `docker compose down -v` удаляет
все данные стенда.

## Работа через веб-интерфейс GitHub

1. Откройте [веб-редактор](https://github.dev/qqrm/openfga-axum-lab),
   создайте ветку, внесите изменения и откройте pull request.
2. В **Checks** у pull request смотрите результаты. Для запуска вручную
   откройте нужный workflow из таблицы, нажмите **Run workflow** и выберите
   ветку. Кнопка доступна пользователям с правом записи в репозиторий.
3. После слияния откройте **Browser preview**, выберите ветку `main` и
   запустите публикацию. Ссылка появится в итогах запуска.

| Workflow | Что запускает |
| --- | --- |
| [CI](https://github.com/qqrm/openfga-axum-lab/actions/workflows/ci.yml) | Rust, Kani, тесты авторизации и восстановления на реальном стенде, сборку WASM-демо |
| [Workflow Security](https://github.com/qqrm/openfga-axum-lab/actions/workflows/workflow-security.yml) | Проверки конфигурации GitHub Actions |
| [CodeQL](https://github.com/qqrm/openfga-axum-lab/actions/workflows/codeql.yml) | Статический анализ кода |
| [Secret scan](https://github.com/qqrm/openfga-axum-lab/actions/workflows/secrets.yml) | Сканирование Git-истории на секреты |
| [Manual performance calibration](https://github.com/qqrm/openfga-axum-lab/actions/workflows/performance.yml) | A/A-замер и выгрузку сырых результатов |
| [Browser preview](https://github.com/qqrm/openfga-axum-lab/actions/workflows/pages.yml) | Сборку, проверку и публикацию демонстрации на Pages |

CI собирает полный Compose-стенд и проверяет права, отказ OpenFGA и
восстановление. Nextest отдельно проверяет инвалидацию кэша на двух
экземплярах Axum с PostgreSQL и OpenFGA в отдельном Compose-проекте.
Проверки работают с вымышленными данными и не требуют личных секретов.

Проверки кэша локально с Docker и `cargo-nextest`:

```sh
cargo nextest run --locked --test cache_replicas --run-ignored only --success-output immediate
```

## Аккаунты и роли

В Compose включены четыре тестовых пользователя с паролем
`local-demo-password`: `alice` — администратор, `bob` — сотрудник,
`carol` — владелец лисы Mika, `dave` — читатель кота Pixel. В интерфейсе
выберите пользователя и введите пароль. Можно задать другой первоначальный
пароль переменной `DEMO_PASSWORD`; после первой инициализации изменение
переменной существующие пароли не меняет.

Приложение проверяет Argon2id-хэши паролей и выдаёт ограниченные по времени
случайные bearer-сессии. Пароли, токены, настройки Compose и отсутствие TLS
предназначены **только для локальной демонстрации**.

## API

1. `POST /session` с JSON `{"username":"alice","password":"..."}`
   возвращает токен; `DELETE /session` отзывает его.
2. Передавайте `Authorization: Bearer <token>` для `GET /pets`,
   `GET /pets/{id}`, `PATCH /pets/{id}` и `DELETE /pets/{id}`.
3. `POST /pets` принимает JSON с `name`, `species`, `biography` и
   обязательный `Idempotency-Key` из 8–128 букв, цифр, дефисов или
   подчёркиваний. Повтор с теми же данными возвращает тот же ID, а с другими
   данными — 409.
4. `PUT` / `DELETE /pets/{id}/relations/{viewer|caregiver}/{user}`
   выдаёт или отзывает доступ. `GET /metrics` отдаёт Prometheus-метрики без
   токенов и идентификаторов пользователей в метках.

При ошибке OpenFGA доступ закрывается ответом 503. Создание и удаление
восстанавливаются после перезапуска; удаление также очищает отношения в
OpenFGA. Точные гарантии и оставшиеся ограничения описаны в
[архитектуре](docs/architecture.md).

## Инженерия и проверки

[INTENT.md](INTENT.md) фиксирует цель, [AGENTS.md](AGENTS.md) — инструкции.
Доменное ядро использует чистые функции и явные типы; Axum служит HTTP-слоем,
а сценарии и интеграции вынесены отдельно. Свойства жизненного цикла проверены
Kani в ограниченной модели. Прохождение модели не является доказательством
всего распределённого сервиса.

CI включает форматирование, Clippy, nextest, cargo-audit/deny/machete,
ShellCheck, actionlint, zizmor, Kani и контейнерные сценарии с отказами.
Намеренно ошибочные примеры проверяют чувствительность ряда инструментов.
Таблица покрытия и пробелов: [проверки](docs/verification.md).

GitHub Dependabot использует расписание из пользовательского шаблона.
Автослияние дополнительно проверяет защиту основной ветки и не включается,
если требуемая защита отсутствует. Сканирование кода CodeQL запускается для
публичной версии репозитория.

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

Короткий тест фиксирует сырые задержки, ошибки и окружение. Отдельный ручной
workflow выполняет рандомизированную A/A-калибровку. На виртуальном runner
это диагностика, а не доказательство регрессии или ёмкости системы:
[протокол](docs/performance.md).

[Варианты запуска в браузере](docs/browser-feasibility.md) рассматриваются
отдельно. Compose-стенд, эмуляция контейнеров и статическая демонстрация имеют
разные гарантии; пока полноценный браузерный стек не подтверждён.
