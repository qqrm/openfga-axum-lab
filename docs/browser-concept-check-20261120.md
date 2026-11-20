# Browser concept: наблюдения 20 ноября 2026

**Реальные действия UI в Chromium выполнились, но общий browser Check завершился ошибкой.**
Причина последнего отказа — поиск CPU-маркера в смешанном логе; результат этого запуска
остаётся `passed: false`. Firefox был пропущен из-за зависимости от Chromium.

## Источник и исходные данные

- Commit: [`1e81518dabdb68c09448eec4b2bdf3cb19c3dac2`](https://github.com/qqrm/openfga-axum-lab/commit/1e81518dabdb68c09448eec4b2bdf3cb19c3dac2).
- Workflow: [run 44686038522](https://github.com/qqrm/openfga-axum-lab/actions/runs/44686038522), attempt 1.
- Chromium: [job 136121743880](https://github.com/qqrm/openfga-axum-lab/actions/runs/44686038522/job/136121743880).
- Raw evidence: [browser-concept-chromium-evidence, artifact 14606424798](https://github.com/qqrm/openfga-axum-lab/actions/runs/44686038522/artifacts/14606424798).
- ZIP: **992 337 bytes**, SHA-256 `29067fd3038bc0959533dace23c8e31ac5ab98c071f6014f1b3caf2a3f377a49`.

Пути далее указаны относительно корня этого ZIP. Результаты относятся к одному CI run.

## Наблюдения в настоящем Chromium

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

## Native proof того же executable

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

## Почему общий gate остался красным

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

Исправление для следующего запуска: отдельное UART-событие
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
Данные синтетические и временные. Успех Firefox и исправленного общего browser gate
будет зафиксирован отдельно только после нового проверенного результата.
