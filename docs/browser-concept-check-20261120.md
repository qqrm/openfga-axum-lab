# Browser concept: наблюдения 20 ноября 2026

## Проверенный результат a6ca0373: native/Chromium PASS, Firefox FAIL

**Native proof и полный Chromium browser Check прошли.** Firefox
([job 136132767623](https://github.com/qqrm/openfga-axum-lab/actions/runs/44688401944/job/136132767623))
завершился по 20-минутному таймауту, поэтому весь workflow имеет итог `failure`.
Для текущего демо подтверждён **Chrome**; успешной приёмки Firefox нет.

- Commit: [`a6ca0373e6cadd6c75354dafcd982bc6350a1f66`](https://github.com/qqrm/openfga-axum-lab/commit/a6ca0373e6cadd6c75354dafcd982bc6350a1f66).
- Workflow: [run 44688401944](https://github.com/qqrm/openfga-axum-lab/actions/runs/44688401944), attempt 1.
- Chromium: [успешный job 136129171047](https://github.com/qqrm/openfga-axum-lab/actions/runs/44688401944/job/136129171047).
- Evidence: [artifact 14608725086](https://github.com/qqrm/openfga-axum-lab/actions/runs/44688401944/artifacts/14608725086), **995 215 bytes**;
  SHA-256 `f1fbeeedab31fafde3feeb47d969c6709300a3415785fe2e9a0bcb6e692caf40`.

Chrome `157.0.8103.78` выполнил два цикла `403 → 204 → 200 → 204 → 403`
через настоящие элементы UI: request IDs `11,13,17,27,31` и `33,35,39,49,53`.
Шесть дополнительных попыток Bob выдать/отозвать доступ вернули `403`.
Всего **54** ответа: 36 × `200`, 4 × `204`, 14 × `403`; `pageErrors: []`.
Guest сохранил `ready: true`, `failed: false` и явное UART-подтверждение
`cpuDiscovery: {"online":"0-1","allowed":"0-1"}`; конфигурация — 2 vCPU / 512 MiB.
От Start (`06:35:39.046Z`) до ready (`06:43:03.460Z`) прошло **7 мин 24,414 с**.
Длительности 54 отдельных HTTP-запросов внутри bridge: **254–669 мс**, медиана
**474 мс**. Одно действие UI может выполнять несколько таких запросов.
Это наблюдение одного CI run, а не end-to-end бенчмарк или время загрузки VM.
Полный capture: **860 batches / 94 352 contiguous chunks / 135 814 bytes** console;
поток заново восстановлен побайтно. Итоговый browser result: **`passed: true`**.

В этом же run native proof снова прошёл с PostgreSQL 17.12, одним документом,
двумя сессиями, двумя циклами и 29 API-наблюдениями. Rust bridge и закрытие доступа
после остановки OpenFGA проверены; `cleanupErrors: []`. ELF и активная модель
побайтно совпадают с указанными ниже: исправление CPU-evidence не меняло авторизацию.

| Запись в evidence ZIP | SHA-256 |
| --- | --- |
| `artifacts/browser-concept-browser/chromium/result.json` | `7f7ed87867d47985af2b4dc7be9f1cbaf310c075de7eaadac525bbc51a510127` |
| `artifacts/browser-concept-native-proof/result.json` | `79615752b8b90fba19beeae6988adae314ae6f778332d10347f00757fefbd982` |

В новом [evidence artifact](https://github.com/qqrm/openfga-axum-lab/actions/runs/44688401944/artifacts/14608725086)
сохранены `before-start.png`, `viewer-granted.png`, `final.png` в
`artifacts/browser-concept-browser/chromium/`.

### Проверенный переносимый bundle

[browser-concept-site-chromium, artifact 14608585369](https://github.com/qqrm/openfga-axum-lab/actions/runs/44688401944/artifacts/14608585369):
**262 693 066 bytes**, SHA-256 `5e7135515ed98d161f4e061c0463be36b7b08d4261b06c161d59858e4f529308`.
Архив распакован в 249 файлов; все **248** записей `assembly.siteFiles` проверены.
Единственное исключение из собственного inventory — `provenance/assembly.json`;
его SHA-256 `753715111b2ddf31ca75cf80f8c5d66350e19c780531f6ee80a317beaea39314`.
Упакованный `serve.mjs` отдельно прошёл smoke: HTML/manifest `GET 200`, Wasm `HEAD 200`,
`POST 405`, отсутствующий файл `404`. Этот smoke не запускал ещё одну VM.
После распаковки: `node /path/to/unpacked-concept/serve.mjs`, затем `http://127.0.0.1:8765/`.
Firefox получил этот же проверенный bundle; результат его отдельной VM приведён ниже.

### Firefox: таймаут до запуска приложения

Firefox **159.0**, тот же commit/run, [evidence artifact 14608708849](https://github.com/qqrm/openfga-axum-lab/actions/runs/44688401944/artifacts/14608708849).
ZIP: **565 997 bytes**, SHA-256 `299d99dcf312ae5b2357f802131bf3c2bb1ccb28543f1d4141b335f7282244d9`.
Восстановление всех 249 файлов точного Chromium bundle подтверждено:
artifact `14608585369`, SHA-256 `5e7135515ed98d161f4e061c0463be36b7b08d4261b06c161d59858e4f529308`;
producer job `136129171047`, отдельный consumer job `136132767623`.

Результат — **`passed: false`**, ошибка `The complete browser concept check exceeded 20 minutes`.
**0 циклов, 0 ответов API**; `guest.ready: false`, `phase`, `cpu`, `health` — `null`.
В логе нет строк `CONCEPT|`; наблюдение не дошло до запуска концепта, Axum или OpenFGA.
Kernel сообщил `smp: Brought up 1 node, 2 CPUs`, но guest UART CPU-подтверждение не поступило.
`pageErrors: []`; полный capture — **230 batches / 19 388 contiguous chunks / 22 456 bytes**,
побайтное восстановление `console.txt` подтверждено.

Последняя kernel-строка: `virtio_blk ... [vda]`, guest time `171.651420`,
получена в `06:47:35.550Z`. Последняя QEMU telemetry — `06:52:16.848Z`;
до завершения в `07:04:26.182Z` новых данных не было **729,334 с**.
Встречаются ACPI `AE_AML_LOOP_TIMEOUT` и PCI `no GSI`; их причинная связь с остановкой
**не доказана**. Это не наблюдавшаяся ошибка OpenFGA Check: HTTP-операции ещё не начались.
Скриншот `artifacts/browser-concept-browser/firefox/final.png` в том же артефакте
показывает STARTING, неизвестные права и недоступные кнопки действий.
SHA-256 `artifacts/browser-concept-browser/firefox/result.json`:
`10e55c82e03ba33c1f5a168383e5be7c90285cd21ab826e1aead35a3c412d56f`.

## История: первый функциональный Chromium run 1e81518d

**Реальные действия UI в Chromium выполнились, но общий browser Check завершился ошибкой.**
Причина последнего отказа — поиск CPU-маркера в смешанном логе; результат этого запуска
остаётся `passed: false`. Firefox был пропущен из-за зависимости от Chromium.

### Источник и исходные данные

- Commit: [`1e81518dabdb68c09448eec4b2bdf3cb19c3dac2`](https://github.com/qqrm/openfga-axum-lab/commit/1e81518dabdb68c09448eec4b2bdf3cb19c3dac2).
- Workflow: [run 44686038522](https://github.com/qqrm/openfga-axum-lab/actions/runs/44686038522), attempt 1.
- Chromium: [job 136121743880](https://github.com/qqrm/openfga-axum-lab/actions/runs/44686038522/job/136121743880).
- Raw evidence: [browser-concept-chromium-evidence, artifact 14606424798](https://github.com/qqrm/openfga-axum-lab/actions/runs/44686038522/artifacts/14606424798).
- ZIP: **992 337 bytes**, SHA-256 `29067fd3038bc0959533dace23c8e31ac5ab98c071f6014f1b3caf2a3f377a49`.

Пути далее указаны относительно корня этого ZIP. Результаты относятся к одному CI run.

### Наблюдения в настоящем Chromium

Chrome `157.0.8103.78`; свежая Linux VM, 2 vCPU, 512 MiB гостевой памяти.
Внутри работают Rust/Axum, PostgreSQL для документа/сессий и OpenFGA с memory datastore.
Playwright переключал Alice/Bob и нажимал кнопки страницы; ответы пришли через Rust HTTP bridge.

| Цикл | Идентификаторы пяти запросов | Фактические HTTP-статусы |
| --- | --- | --- |
| 1 | `11, 13, 17, 27, 31` | `403 → 204 → 200 → 204 → 403` |
| 2 | `33, 35, 39, 49, 53` | `403 → 204 → 200 → 204 → 403` |

Последовательность: Bob не читает; Alice выдаёт `viewer`; Bob читает; Alice отзывает;
Bob снова не читает. Дополнительно **шесть** попыток Bob выдать/отозвать доступ вернули
`403`: запросы `3, 7` до выдачи и `19, 23, 41, 45` при наличии `viewer`.
UI-проверки тела документа, истории ответов и плашек прав завершились успешно.
Итог: Alice может читать и делиться, Bob не может ни читать, ни делиться.

Сохранены **54** последовательных ответа: 36 × `200`, 4 × `204`, 14 × `403`;
`pageErrors: []`, guest `ready: true`, `failed: false`.
От нажатия Start (`06:15:56.469Z`) до готовности (`06:24:50.136Z`) прошло **8 мин 53,667 с**.
Измерения HTTP bridge для этих 54 запросов: **0,287–0,988 с**. Это время внутри bridge
в одном запуске, без загрузки VM; оно не является браузерной end-to-end оценкой или бенчмарком.

Скриншоты находятся в `artifacts/browser-concept-browser/chromium/`:

| Файл в evidence ZIP | Состояние страницы |
| --- | --- |
| [before-start.png](https://github.com/qqrm/openfga-axum-lab/actions/runs/44686038522/artifacts/14606424798) | До запуска VM |
| [viewer-granted.png](https://github.com/qqrm/openfga-axum-lab/actions/runs/44686038522/artifacts/14606424798) | Bob получил документ после выдачи доступа |
| [final.png](https://github.com/qqrm/openfga-axum-lab/actions/runs/44686038522/artifacts/14606424798) | После второго отзыва и финального отказа Bob |

Ссылки открывают общий артефакт; имена выбираются после распаковки ZIP.

### Native proof того же executable

`artifacts/browser-concept-native-proof/result.json`: **`passed: true`**, SHA-256
`aeab4660fa55a2c47c9f7ca9fc8494ad5399eebf05ff2a84616c2a811cbaf571`.
Настоящий PostgreSQL **17.12**, одна SQL-запись документа и две сессии; реальные OpenFGA и Axum.
29 API-наблюдений включают два полных цикла, шесть отказов Bob в изменениях,
два `401`, два `400` и два `503` после остановки OpenFGA. Rust bridge отдельно подтвердил
готовность, чтение Bob `403` и права Alice `200`; `cleanupErrors: []`.

ELF `concept`: 6 764 512 bytes, SHA-256 `58332b8857b6b9ad1058f5193d256aa00f5efc00191b7893faa76d400dbe1c16`.
Активная модель: 849 bytes, SHA-256 `8d82f5df2d5744902d6725f9bff05bc94b64382764359bdc3998d5e0010bd5dd`.
Она содержит только `user`/`document`, `owner`/`viewer`, `can_read` и `can_share`.
Исходная Lakekeeper-фикстура прогрева кодеков OpenFGA остаётся отдельной от активной модели.

### Почему общий gate остался красным

Единственная итоговая ошибка: `The guest did not provide actual two-CPU discovery evidence`.
Checker искал целую строку `[concept-guest] online CPUs: 0-1; service CPUs: 0-1`
в `console.txt`. QEMU вставил telemetry между UART-символами слова `CPUs`, поэтому
строки целиком в смешанном представлении нет.

Raw `stream.jsonl` содержит **973 batches / 94 531 contiguous chunks** и точно
восстанавливает **141 284 bytes** `console.txt`. После отделения 150 целых QEMU-чанков
UART-поток содержит полный ожидаемый CPU-маркер. Kernel также сообщил
`smp: Brought up 1 node, 2 CPUs`; guest прошёл обязательные проверки CPU online и
`Cpus_allowed_list` на `0-1` до запуска сервисов. Это подтверждает два CPU в этом run,
но не меняет сохранённый результат автоматического gate.

Исправление, затем проверенное в a6ca0373: отдельное UART-событие
`CONCEPT|cpu|{"online":"0-1","allowed":"0-1"}`, обязательное до `ready`, и проверка
его структурированного состояния вместо поиска строки в смешанном логе.
Модель, авторизация и ожидаемые HTTP-статусы этим исправлением не меняются.

Browser result SHA-256: `30a8a4cb3dedbfe78c01fea499299bf368aa3ace02800b644c3394b440d79d56`.
Console SHA-256: `27cd0e5879d8c18f4f648faae6007c3bf724ba114f8ca342c2282f9f01aa9ed6`.
Stream SHA-256: `b6ffa480f0d3294cf90e5ba496c6779a9dd0cb80c80ed0a01d6c727d5b3827a1`.

## Граница результата

Это [отдельный маленький концепт](../browser-concept/README.md), не полный Lakekeeper workload.
Его миграция, матрица и `lab-tool acceptance` здесь не запускались;
`lakekeeperAcceptance: false`, `productionAcceptance: false`.
Данные синтетические и временные. Подтверждённый Chromium относится только к этому концепту.
Firefox acceptance не пройден; успешные native/Chrome результаты не превращают весь workflow в успешный.
