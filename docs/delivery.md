# Поставка, staging и stable

## Фактическая граница

Подготовлены image-only Compose, инструменты приёмки и workflows. Наличие этих
файлов не означает опубликованного образа, релиза или постоянного backend.
До настройки хоста и защищённых GitHub Environments удалённый deployment
блокируется. Pages — только статическая демонстрация.

Образ содержит `app`, `lab-tool`, SQL-миграции и FGA-модель/tuples. Bundle
содержит `compose.release.yaml`, `db/init.sql`, README, этот документ,
`manifest.json`, `images.env`; пароль или session token в bundle не входят.
Поддерживаемая платформа первого pipeline: Linux x86_64 (Docker/WSL2).

## Установка опубликованного bundle

Скачайте `delivery-vX.Y.Z.tar.gz`, `manifest.json`, `SHA256SUMS` одного и того же
проверенного выпуска в пустой каталог. Название здесь — шаблон, не существующий
релиз. Проверьте файл checksum, затем распакуйте архив. Для приватного GHCR
сначала выполните login учётной записью с `read:packages`; анонимный pull пока
не заявляется и должен быть проверен после настройки видимости пакета.

```sh
sha256sum --check SHA256SUMS
tar -xzf delivery-vX.Y.Z.tar.gz
image=$(sed -n 's/^LAB_IMAGE=//p' images.env)
docker pull "$image"
docker run --rm --user "$(id -u):$(id -g)" --env-file images.env \
  -v "$PWD:/work" -w /work --entrypoint /usr/local/bin/lab-tool \
  "$image" init-env local "$image"
docker compose -p authzlab-local --env-file local/environment.env -f compose.release.yaml up -d --wait app
docker compose -p authzlab-local --env-file local/environment.env -f compose.release.yaml run --rm acceptance
```

Пример предполагает, что имя архива заменено фактическим. UI слушает только
loopback-порт 3000. Первоначальный пароль доступен локально в
`local/secrets/demo-password`; не прикладывайте его к issue или логам.
`init-env` генерирует разные DB/demo-пароли, не перезаписывает существующий
каталог и не вращает пароли существующей БД. Файлы секретов читаются из mounts,
а не передаются в command line. На Unix внешний каталог имеет режим 0700;
отдельные файлы, монтируемые для UID 65534, должны быть читаемыми контейнером.
На Windows используйте WSL2 и убедитесь в ограничении доступа ACL.

У `compose.release.yaml` нет `build`: неполная поставка не компенсируется
незаметной сборкой исходников. PostgreSQL и OpenFGA из опубликованного bundle
закреплены digest; локальный `init-env` без `images.env` допускает известные
version tags зависимостей и не обещает побитового воспроизведения.

## Два локальных окружения

Повторите `init-env` с каталогами `staging` и `stable`, каждый раз отдельно.
Не копируйте `secrets/` между ними. Запуск из корня распакованного bundle:

```sh
APP_PORT=3001 docker compose -p authzlab-staging --env-file staging/environment.env -f compose.release.yaml up -d --wait app
APP_PORT=3000 docker compose -p authzlab-stable --env-file stable/environment.env -f compose.release.yaml up -d --wait app
```

Разделены Compose project, сети, оба тома, обе базы, FGA store/model и пароли.
Порт API задаётся отдельно. В одном окружении PostgreSQL содержит две базы;
между окружениями сам PostgreSQL также отдельный. Нельзя задавать общие
external volumes или подключать benchmark к этим данным.

## Выпуск без пересборки

1. PR проходит обычный CI, Workflow Security, Secret scan и Delivery verification.
   После merge нужны успешные проверки именно нового SHA в `main`, не старого
   PR head/merge ref. Рекомендуется добавить `delivery` в branch protection;
   release policy проверяет его даже до изменения административных настроек.
2. Настройте Environment `release`: protected branches only и required reviewer.
   Workflow **Prepare verified release candidate**, запуск только с `main`,
   требует tag, совпадающий с Cargo version (возможен суффикс `-rc.N`).
3. Pipeline собирает образ один раз, тестирует его на двух одноразовых стендах,
   публикует в GHCR, удаляет локальные теги, заново вытягивает по digest и повторно
   проверяет. Создаётся **draft** GitHub Release с manifest и полным bundle.
4. **Deploy verified environment → staging** принимает этот digest и записывает
   receipt только после реального сценария авторизации и проверки HTTPS-адреса.
5. **Publish staged release** требует успешный staging receipt с теми же tag,
   source SHA и digest; подтверждение владельца задаёт protected Environment.
   Сборки в этом workflow нет.
6. **Deploy verified environment → stable** допускает только опубликованный
   релиз с совпадающим staging receipt и снова проверяет работающий сервис.

Rust-политика проверяет последний результат каждого нужного workflow для
точного SHA. Новый failed/in-progress запуск не маскируется старым success.
Пустые/неверные Environment rules и отсутствующее review для stable/release
останавливают процесс. Требуется сохранить защиту `main`; код workflow не
изменяет ни protections, ни Environments автоматически.

GitHub release assets и GHCR доступны только в рамках действующих прав аккаунта.
Checksums проверяют целостность, не заменяют доверие к правам публикации.
Автоматическая публикация из произвольного tag push удалена. Никаких
самоподписанных заявлений о production readiness или SBOM здесь нет.

## Предпосылки реального deployment

Нужен выделенный доверенный Linux x86_64 runner с метками
`self-hosted, linux, x64, authzlab-deploy`, Docker Compose v2, `gh`, `jq`, `curl`
и доступом к `/srv/authzlab`. На нём **не выполняются PR jobs**. Repo variable
`AUTHZLAB_REMOTE_DEPLOY_ENABLED=true` включается владельцем только после
подготовки; без него hosted preflight падает до постановки задачи на этот runner.

В Environments `staging`, `stable`: protected branches only; у stable ещё
required reviewer. Каждый задаёт `AUTHZLAB_URL` — фактический ограниченный
HTTPS-адрес без credentials в URL. Reverse proxy/TLS и контроль доступа
настраиваются владельцем хоста, не имитируются в этом PR. Установите блокировку
доступа из публичного интернета либо внешний аутентифицированный доступ;
демо не рассчитано на произвольных посетителей.

На хосте отдельно создайте `/srv/authzlab/staging/state` и
`/srv/authzlab/stable/state` через `lab-tool init-env` выбранного образа. Секреты
должны быть независимыми; предварительно создайте родительские каталоги и
права для оператора. Deployment требует уже существующий `state/environment.env`
и не генерирует новые пароли поверх существующих БД. Внешняя root директория
должна быть доступна только оператору; содержимое `state` не коммитится.

Deployment сохраняет bundle в `releases/<tag>`, а последнюю и предыдущую
успешные идентичности — в `last-successful.json`/`previous-successful.json`.
Сервис по-прежнему слушает только 127.0.0.1:3001 или :3000. Конкурирующие
публикации в одно окружение сериализованы. Ошибка smoke/HTTPS проверки остаётся
ошибкой job: наличие контейнера не считается успешным deployment.

## Резервная копия и откат

Перед обновлением остановите приём запросов и всех писателей; остановите app
и OpenFGA **нужного окружения**, но оставьте PostgreSQL доступным для dump.
Согласованно сохраните обе базы (`authzlab`, `openfga`), `/data/config.json` из
тома `fga-state`, точный manifest/digests и секреты в отдельное защищённое
хранилище. SQL dumps содержат персональные данные и хэши сессий — не загружайте
их как публичные GitHub artifacts. Не копируйте физические файлы активной БД.

`tests/delivery.rs` выполняет согласованный dump/restore двух баз на одноразовом
стенде, проверяет исходные данные, store/model, старую сессию и повторную
приёмку. Это не проверка disaster recovery на новом физическом хосте и не
замена независимому backup config volume. Периодический restore drill на
новом хосте остаётся операторской задачей.

Откат приложения = повторный deployment ранее проверенного digest **только
при совместимой схеме PostgreSQL и FGA-модели**. При несовместимости нужен
согласованный restore всей пары БД плюс config/секреты. Не выполняйте down -v
ради отката. Нельзя обещать откат данных простым запуском старого контейнера.
Workflow требует явного подтверждения compatibility/backup; он не способен
сам доказать совместимость произвольной будущей миграции и не делает
автоматический разрушительный rollback.

## Troubleshooting

* Занят порт: измените `APP_PORT` у локального окружения; persistent workflow
  использует выделенные 3001/3000, это нужно учесть при подготовке хоста.
* `healthz=200`, `readyz=503`: проверьте логи app/OpenFGA/PostgreSQL, существование
  сохранённых store/model и монтирование секретов. Не удаляйте config для
  «починки»: bootstrap намеренно отказывает при потере существующего store.
* Повторный `init-env` отказал: это защита от перезаписи. Используйте существующий
  файл; изменение DEMO_PASSWORD не меняет уже созданные пароли.
* Нет draft/release/доступа GHCR: проверьте точный SHA CI, review Environment,
  разрешения `packages:write`/`read:packages` и видимость пакета. Не заявляйте
  публичный pull по успешному pull с авторизацией.
* Проверки находятся в Actions run → Artifacts. В `delivery-evidence.json`
  описан именно выполненный тест, а не факт удалённого deployment.
